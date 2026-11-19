# Signed and notarized builds

Lumora's CI can sign the app with a **Developer ID** certificate and have
Apple **notarize** it. The pipeline is optional: it switches on only when the
secrets below exist in the repository. Without them CI keeps building the
same ad-hoc signed DMG as before and says so in the run's annotations.

## Why it matters

macOS remembers privacy grants (Screen Recording, Microphone, Speech
Recognition, Accessibility / Input Monitoring) and Keychain access by the
app's **designated requirement**, the rule in its code signature that says
"this is the same app".

- **Ad-hoc builds** (today's pre-releases) have no certificate, so their
  requirement is the hash of this exact build (`cdhash H"…"`). Every update
  is a different app to macOS: the old grant stops applying and the user has
  to remove and re-add Lumora in System Settings.
- **Developer ID builds** get the requirement

  ```
  identifier "com.lumora.app" and anchor apple generic and
  certificate 1[field.1.2.840.113635.100.6.2.6] and
  certificate leaf[field.1.2.840.113635.100.6.1.13] and
  certificate leaf[subject.OU] = <TEAM_ID>
  ```

  which is identical for every build signed by the same team, so grants
  survive updates. `scripts/bundle.sh` pins the identifier to
  `com.lumora.app` and refuses to finish a Developer ID build whose
  requirement does not name the bundle identifier and team (or names a
  `cdhash`). Keep `CFBundleIdentifier` at `com.lumora.app`; the script also
  refuses a plist that changed it.
- **Notarization** lets Gatekeeper open the app without the "unidentified
  developer" / "Apple could not verify" dialog. The ticket is stapled to
  both the app and the DMG, so it also works offline.

The first Developer ID build is still a new identity compared with the
ad-hoc ones, so users grant permissions once more after installing it, and
then never again for later signed updates.

## What you need

A paid **Apple Developer Program** membership (individual or organisation).
Free Apple accounts cannot create Developer ID certificates or notarize.

## Secrets

Add these in **GitHub → the repository → Settings → Secrets and variables →
Actions → New repository secret**.

### Signing (all three required)

| Secret | Value |
| --- | --- |
| `MACOS_CERT_P12` | The **Developer ID Application** certificate *with its private key*, exported as `.p12`, then base64-encoded (see below). |
| `MACOS_CERT_PASSWORD` | The password you chose when exporting the `.p12`. |
| `APPLE_TEAM_ID` | Your 10-character Team ID (developer.apple.com → Account → Membership details). |

If only some of these are set, CI warns and builds ad-hoc.

### Notarization (choose one)

**App Store Connect API key (recommended)** — not tied to a person's
password or two-factor login:

| Secret | Value |
| --- | --- |
| `NOTARY_KEY_ID` | The key's ID (e.g. `ABC123DEFG`). |
| `NOTARY_ISSUER_ID` | The issuer ID shown above the keys list (a UUID). |
| `NOTARY_KEY_P8` | The text of the downloaded `AuthKey_<ID>.p8` file (the whole `-----BEGIN PRIVATE KEY-----` block), or base64 of it. |

Create it at App Store Connect → Users and Access → Integrations → **Team
Keys** → “+”, role **Developer**. The `.p8` can be downloaded only once.

**Apple ID + app-specific password**:

| Secret | Value |
| --- | --- |
| `APPLE_ID` | The Apple ID email of a team member. |
| `APPLE_APP_PASSWORD` | An **app-specific password** from appleid.apple.com → Sign-In and Security → App-Specific Passwords (not the account password). |

When both sets are complete the API key is used.

With the signing secrets but no notary secrets, branch and PR builds are
Developer ID signed (stable identity) but **not notarized**, and CI warns;
Gatekeeper will block their first open. Tag builds (`v*`) fail instead
(`REQUIRE_NOTARIZATION=1`), so a release can't go out un-notarized.

## Exporting the certificate

1. On a Mac, create the certificate if you don't have one:
   Xcode → Settings → Accounts → your team → **Manage Certificates…** → “+” →
   **Developer ID Application**. (Or developer.apple.com → Certificates →
   “+” → Developer ID Application, with a CSR from Keychain Access.)
2. Open **Keychain Access** → login keychain → **My Certificates**. Find
   “Developer ID Application: Your Name (TEAMID)”; expand it to check the
   private key is underneath.
3. Right-click the certificate (not the key) → **Export…** → format
   **Personal Information Exchange (.p12)** → choose a strong password.
4. Base64-encode it and copy it to the clipboard:

   ```sh
   base64 -i DeveloperID.p12 | pbcopy
   ```

   Paste that as `MACOS_CERT_P12`, and the export password as
   `MACOS_CERT_PASSWORD`. Then delete the `.p12` file.

“Apple Development” and “Apple Distribution” certificates will not work:
only **Developer ID Application** certificates can be notarized for
distribution outside the Mac App Store. The script checks this and that the
certificate belongs to `APPLE_TEAM_ID`.

## What CI does

`scripts/sign-and-notarize.sh` (the CI build step) with the secrets present:

1. Creates a temporary keychain with a random password, imports the `.p12`,
   allows `codesign` to use the key, and adds the keychain to the search
   list.
2. Finds the “Developer ID Application: … (TEAM_ID)” identity.
3. Stores the notary credentials as a `notarytool` profile in that keychain,
   then removes the raw secrets from the environment before building.
4. Runs `scripts/bundle.sh`, which builds the universal app and:
   - `codesign --options runtime --timestamp --identifier com.lumora.app
     --entitlements packaging/entitlements.plist` (hardened runtime; the only
     entitlement is `com.apple.security.device.audio-input` for the
     microphone; no App Sandbox);
   - checks the designated requirement (bundle ID + team, no cdhash);
   - zips the app, `xcrun notarytool submit --wait`, and fails with Apple's
     log unless the status is `Accepted`; `xcrun stapler staple` +
     `stapler validate`; `spctl --assess --type execute`;
   - builds the DMG, signs it, notarizes and staples it, and checks it with
     `spctl --assess --type open --context context:primary-signature`.
5. Runs `codesign --verify --deep --strict` on the app and DMG, prints the
   signing authority and team, and writes a line to the job summary.
6. Deletes the temporary keychain and restores the keychain search list,
   whether the build passed or failed.

The DMG artifact keeps its name (`Lumora-dmg` → `Lumora-<version>.dmg`).
No secret value is ever echoed; GitHub also masks them in logs. Pull
requests from forks never receive secrets, so they build ad-hoc.

## Signing locally

```sh
# one-time: store notary credentials in your login keychain
xcrun notarytool store-credentials lumora-notary \
  --apple-id you@example.com --team-id TEAMID --password app-specific-password

SIGN_IDENTITY="Developer ID Application: Your Name (TEAMID)" \
APPLE_TEAM_ID=TEAMID NOTARY_PROFILE=lumora-notary scripts/bundle.sh
```

Or export the same variables as CI and run `scripts/sign-and-notarize.sh`.

## Checking a build

```sh
codesign -dv --verbose=4 /Applications/Lumora.app   # Authority=Developer ID Application…, TeamIdentifier
codesign -d -r- /Applications/Lumora.app            # designated => identifier "com.lumora.app" and anchor apple generic …
spctl -a -vv /Applications/Lumora.app               # accepted, source=Notarized Developer ID
xcrun stapler validate /Applications/Lumora.app
```

## Troubleshooting

- **“notarization … was not accepted (Invalid)”** — the log printed below
  it lists each problem (missing timestamp, hardened runtime off, unsigned
  nested code…).
- **`errSecInternalComponent` from codesign** — the key's partition list was
  not set or the keychain locked; re-run the job.
- **“not a valid Developer ID Application identity”** — the `.p12` holds a
  different certificate type, no private key, or another team's certificate.
- **Permissions asked again after the first signed update** — expected once
  when moving from ad-hoc to Developer ID; later signed updates keep them.

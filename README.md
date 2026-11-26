# OVC

OVC is a desktop version-control tool for Office files. It is now built with a
Rust core and a Tauri desktop GUI, with a standalone command-line interface
for local automation.

OVC supports `.docx`, `.xlsx`, and `.pptx`. It stores Office files as
content-addressed package parts, so unchanged internal files can be reused
between versions. It also creates text snapshots for readable diffs.

## Features

- Tauri desktop app for Windows/Linux/macOS builds
- Office add-in task pane for Word, Excel, and PowerPoint
- Local `.office-vcs/` repository in each workspace
- Automatic detection of changed tracked files
- Commit detected changes without manually adding files again
- History, latest diff, and checkout/restore
- Deduplicated blob storage for Office package contents
- HTTPS local API for the Office add-in at `https://localhost:38655`

## Requirements

- Rust stable
- Cargo
- Tauri CLI 2.x
- Platform dependencies for Tauri

Install the Tauri prerequisites for your platform before building.

## Development

Run the desktop app:

```bash
cd src-tauri
cargo tauri dev
```

Check Rust compilation:

```bash
cd src-tauri
cargo check
```

Format Rust code:

```bash
cd src-tauri
cargo fmt
```

Build the desktop app:

```bash
cd src-tauri
cargo tauri build
```

On Windows this creates an NSIS installer. Build output is written under:

```text
src-tauri/target/release/bundle/nsis/
```

When cross-compiling from Linux for Windows:

```bash
cd src-tauri
cargo tauri build --target x86_64-pc-windows-gnu --bundles nsis
```

The Windows installer is written under:

```text
src-tauri/target/x86_64-pc-windows-gnu/release/bundle/nsis/
```

## Desktop App Usage

1. Open **Workspace → Choose folder**.
2. Use **Workspace → Initialize repository** to create `.office-vcs/`.
3. Use **Workspace → Add files** to track and stage `.docx`, `.xlsx`, or `.pptx` files.
4. After editing a tracked file, use **Stage** beside it under **Changes**.
5. Enter a commit message and click **Commit staged**.

**Stage** captures the saved file at that moment. Further edits appear under
**Changes** until you stage them again. Click a file under **Staged** to compare
that snapshot with its latest commit, or under **Changes** to compare the saved
file with its staged snapshot or latest commit. Use **Text** and **Images** to
inspect the differences.

### AI commit messages

Open **Workspace → AI API settings** and choose OpenAI Chat Completions or
Anthropic Messages. Enter an API base URL, model name and API key. Enable
**Model supports images** to include supported changed-image previews.
With files staged, click **AI draft** to generate a commit message for review.
OVC sends excerpts of staged changes to the configured endpoint and fills the
commit message field when it receives a response.

The settings and API key are saved in the app's user configuration directory.
On Windows the default path is `%APPDATA%\com.ovc.desktop\ai-settings.json`.
The API key is stored in plain text. The settings dialog shows the exact path.

## Office Add-in

The add-in files live in:

```text
office-addin/
```

The manifest is:

```text
office-addin/manifest.xml
```

The Tauri desktop app starts a local HTTPS API automatically:

```text
https://localhost:38655
```

Before sideloading the add-in on Windows, run the setup script:

```text
The Windows installer runs this automatically when possible.
```

If Office reports a localhost or certificate error, run the one-time setup from
an Administrator PowerShell window:

```powershell
Set-ExecutionPolicy -Scope Process -ExecutionPolicy Bypass
.\office-addin\setup-office-addin.ps1
```

This installs the trusted certificate, adds the Office WebView localhost loopback
exemption, creates a local trusted add-in catalog at
`\\<your-computer-name>\OVCOfficeAddinCatalog`, and registers OVC in Office.

### Loading the add-in in Office

After running the installer (or `setup-office-addin.ps1`), restart Word, Excel,
or PowerPoint, then load OVC using one of the methods below.

**Method A — My Add-ins (recommended for Microsoft 365)**

1. Open any Word, Excel, or PowerPoint document.
2. Click the **Insert** tab in the ribbon.
3. Click **Add-ins** (or **Get Add-ins**).
4. In the dialog that opens, choose **My Add-ins** from the top.
5. Scroll to find **OVC**, then click it to insert the task pane.

If OVC does not appear under My Add-ins, try the **Shared Folder** tab
(visible when the catalog is registered) or use Method B below.

**Method B — Add from file (one-time manual load)**

1. Open any Word, Excel, or PowerPoint document.
2. Click **Insert → Add-ins → More Add-ins** (or **Get Add-ins**).
3. In the Office Add-ins dialog, click **Upload My Add-in** or choose
   **Add a custom add-in → Add from file…**
4. Browse to `office-addin\manifest.xml` inside the OVC installation folder
   (default: `C:\Program Files\OVC\resources\office-addin\manifest.xml`)
   or the cloned repository root.
5. Click **Open**. The OVC task pane opens on the right side of the document.

> **Note:** OVC must be running before you open the add-in. If Office shows a
> certificate or connection error, open the OVC desktop app first, then reload
> the add-in.

### Pinning OVC for quick access

After inserting the task pane for the first time, it reopens automatically the
next time you open the same document. To add OVC to the ribbon permanently,
right-click the task pane title bar and choose **Attach to Ribbon**.

Current add-in features:

- Detect current local Office document path when Office exposes it
- Save the current document as a new OVC version from its saved contents. The
  operation stages and commits that document only, leaving other staged files
  and edits available in the desktop app.
- Show version history for the current document

If Office cannot expose the local file path, paste the file path manually in the
task pane.

## Command line

The Windows installer installs the desktop app and the `ovc` command together.
After installation, open a new PowerShell or Command Prompt window:

```powershell
ovc --help
ovc --workspace "D:\Documents" status
ovc --workspace "D:\Documents" tracked-files
ovc --workspace "D:\Documents" diff-working "Report.docx"
```

The installer adds the OVC `bin` directory to the machine PATH and removes
that entry on uninstall. The command uses the same `.office-vcs/` workspace
as the desktop app and runs without launching it.

When developing from source, build or run the CLI target with Cargo:

```bash
cargo build --manifest-path src-tauri/Cargo.toml --release --bin ovc-cli
cargo run --manifest-path src-tauri/Cargo.toml --bin ovc-cli -- --help
```

For a standalone build from source, the release executable is
`src-tauri/target/release/ovc-cli` (`ovc-cli.exe` on Windows). Use the workspace
directory containing `.office-vcs/` as `--workspace`.

```bash
ovc-cli --workspace "/path/to/workspace" status
ovc-cli --workspace "/path/to/workspace" tracked-files
ovc-cli --workspace "/path/to/workspace" log "/path/to/document.docx"
ovc-cli --workspace "/path/to/workspace" diff-latest "/path/to/document.docx"
```

Use the `sourcePath` from `tracked-files` for version queries and the version
IDs returned by `log` for `diff-versions` and `checkout`. The CLI also
supports `init`, `track` and `commit`. Successful commands produce JSON on
stdout. Errors go to stderr and return a nonzero exit code. Checkout
overwrites an existing file at the supplied destination.

`diff-working <file>` compares the saved file with its latest committed
version, or with an empty baseline before its first commit.
`stage <file>` captures a snapshot and `unstage <file>` removes it from the
staging area. Use `diff-staged <file>` to review the snapshot and
`commit-staged <message>` to commit the staging area. Use
`diff-unstaged <file>` to review edits made since staging.

## Repository Data

Each workspace stores runtime data in `.office-vcs/`:

```text
.office-vcs/
├── metadata.json
├── blobs/
├── packages/
├── objects/
└── snapshots/
```

- `metadata.json` records tracked files, versions, hashes, messages, and paths
- `blobs/` stores deduplicated files from inside Office zip packages
- `packages/` stores one manifest per saved version
- `objects/` is kept for compatibility with older storage layouts
- `snapshots/` stores extracted text used by diff

Do not commit `.office-vcs/` to Git.

## Project Structure

```text
.
├── app/
│   ├── index.html
│   ├── styles.css
│   └── app.js
├── office-addin/
│   ├── certs/
│   ├── manifest.xml
│   ├── taskpane.html
│   ├── taskpane.css
│   └── taskpane.js
├── src-tauri/
│   ├── Cargo.toml
│   ├── tauri.conf.json
│   ├── icons/
│   └── src/
│       ├── main.rs
│       ├── core.rs
│       ├── office.rs
│       └── api.rs
└── README.md
```

## Certificates

The HTTPS server at `https://localhost:38655` uses a self-signed TLS certificate.
Certificates are generated on the user's machine during installation by
`setup-office-addin.ps1`, which calls the OVC executable's hidden
`--generate-office-certs` command. The repository and installer do not ship a
shared private key.

| File | Purpose | Committed |
|---|---|---|
| `ovc-localhost-ca.crt` | CA certificate generated at install time | ❌ no — generated locally |
| `ovc-localhost-ca.key` | CA private key generated at install time | ❌ no — generated locally |
| `ovc-localhost.crt` | Server certificate generated at install time | ❌ no — generated locally |
| `ovc-localhost.key` | Server private key | ❌ no — in `.gitignore` |

### First-time setup after cloning

After cloning the repository, generate certificates and register the add-in with:

```powershell
Set-ExecutionPolicy -Scope Process -ExecutionPolicy Bypass
.\office-addin\setup-office-addin.ps1
```

### After regenerating certificates

New certificates have a different CA fingerprint, so Windows will no longer trust
the old installed CA. Run the following in an Administrator PowerShell window:

```powershell
Set-ExecutionPolicy -Scope Process -ExecutionPolicy Bypass
.\office-addin\uninstall-office-addin-cert.ps1
.\office-addin\setup-office-addin.ps1
```


## Limitations

OVC targets modern Office Open XML files only: `.docx`, `.xlsx`, and `.pptx`.
Older binary Office formats such as `.doc`, `.xls`, and `.ppt` are not
supported.

Text diff is best-effort. Restored Office files preserve document contents, but
their zip byte layout may not be byte-for-byte identical to the original because
the package is reconstructed from stored parts.

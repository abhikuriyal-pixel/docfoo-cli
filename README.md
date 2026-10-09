# DocFoo CLI

**From documents to grounded answers — in your terminal or application.**

DocFoo CLI scans PDFs/images into a local Markdown library, builds scoped knowledge graphs and answers questions with citations and figures. Scriptable commands and structured JSON support application and agent integrations, including PiMinions and Hermes/Slack.

[Download releases](https://github.com/abhikuriyal-pixel/docfoo-cli/releases) · [Report a problem](https://github.com/abhikuriyal-pixel/docfoo-cli/issues) · [Support](SUPPORT.md)

> **Repository scope**
> This repository currently hosts documentation, installer bootstraps and binary releases. Rust/sidecar implementation and ongoing development history are maintained privately; no source-publication date is committed. Previously published MIT source retains its existing license rights.

## Install

Downloads require no GitHub login. Archives contain both `docfoo` and the matching `docfoo-agent` model sidecar; keep them together.

### Windows x64

Download `docfoo-cli-<version>-windows-x64.zip` and its `.sha256` sidecar from the selected [release](https://github.com/abhikuriyal-pixel/docfoo-cli/releases). Verify before extracting:

```powershell
Get-FileHash .\docfoo-cli-<version>-windows-x64.zip -Algorithm SHA256
```

Compare with the `.sha256` file, extract the folder and run `docfoo.exe version`. Alternatively, review and download [install.ps1](install.ps1), then execute it in PowerShell; it verifies the archive, installs to `%USERPROFILE%\.docfoo\bin`, adds that directory to user PATH and provisions scan dependencies. Executables are unsigned.

```powershell
Invoke-WebRequest https://raw.githubusercontent.com/abhikuriyal-pixel/docfoo-cli/main/install.ps1 -OutFile install.ps1
# Review the script before running it.
.\install.ps1
```

### Linux / WSL x64

Use a release that actually includes `docfoo-cli-<version>-linux-x64.tar.gz` and its `.sha256` sidecar; some historical versions are Windows-only. Release CI targets Linux x64 on Ubuntu 22.04, not every Linux distribution. macOS/ARM builds are not offered.

```bash
sha256sum -c docfoo-cli-<version>-linux-x64.tar.gz.sha256
tar -xzf docfoo-cli-<version>-linux-x64.tar.gz
```

For an installer, review [install.sh](install.sh), then explicitly select a release with Linux assets:

```bash
curl -fsSL https://raw.githubusercontent.com/abhikuriyal-pixel/docfoo-cli/main/install.sh -o install.sh
# Review before executing; replace <version> with an available Linux release.
bash install.sh --version <version>
```

All default branches are `main`. Old raw `/master/install.*` URLs are no longer supported.

## Quick start

```bash
docfoo version --json
docfoo doctor              # available in v0.2.1 and newer
docfoo setup
docfoo auth --set PROVIDER
docfoo model --list
# Replace these placeholders with available catalog keys:
docfoo model --set scan provider/vision-model
docfoo model --set kg provider/answer-model
docfoo scan paper.pdf --output papers
docfoo kg --index --scope papers
docfoo kg --query "Summarize the key findings" --scope papers --json
docfoo update --check
```

Configure suitable scanning/answering models using `docfoo model --set SLOT provider/model`; use `docfoo help` for options. Default workspace: `~/.docfoo` (`%USERPROFILE%\.docfoo` on Windows). `docfoo setup` downloads pinned, checksummed ONNX Runtime, PDFium and the layout model. The public `deps-v1` model asset is a setup dependency, not the latest CLI version.

## Workflows and help

| Task | Commands |
| --- | --- |
| Document library | `scan`, `resources`, `notes` |
| Knowledge retrieval | `kg --index`, `kg --query`, `kg --status` |
| Backup and sharing | `backup`, `restore`, `collections` |
| Configuration and health | `model`, `auth`, `setup`, `doctor` |
| Application | `version`, `update`, `completions` |

Use `docfoo <command> --help` for options. Add `--json` for application/script integration; human summaries are not a parsing API. Keep backup files private and outside the data being backed up. Close desktop/library writers before restore.

**New in v0.2.1:** grouped help, local read-only `doctor` checks, secure `auth --key-stdin`, consistent diagnostics, safer backup/restore/collection replacement and stricter flag/path validation. Action-flag syntax, the `docfoo.cli/1` envelope and existing scan model aliases are retained. Upgrade older versions before using the new options. JSON/non-interactive restore requires `--yes`, and replacing an installed graph collection requires `--force`. On Windows, self-update stages `.exe.new` files; stop both processes and replace the matching binary pair to finish installation.

## Privacy and release policy

The library is local, but configured model providers receive document/query content and may charge for usage. Authentication files and document libraries must be treated as sensitive. Do not post credentials, private documents, answers or workspace backups in public issues.

Archives and SHA-256 sidecars are immutable by policy; migrated versions retain their original bytes and notes. GitHub's automatic **Source code** archives contain documentation and bootstraps, not CLI implementation. `DOCFOO_REPO` can point installers/updaters at a compatible release mirror.

## License and support

[MIT License](LICENSE). Third-party native libraries/models and bundled dependencies retain their own licenses. Historical artifacts are not rebuilt to add new files. See [SUPPORT.md](SUPPORT.md) and [SECURITY.md](SECURITY.md).

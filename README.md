# DocFoo CLI

Scan PDFs and images into a local Markdown library, then ask questions with citations back to your documents.

DocFoo extracts text, tables, and figures, organizes documents into collections, and builds knowledge graphs for searching across them. You can also browse your library, read notes, and make or restore backups from the terminal. Commands support JSON output for scripts and other applications.

[Download](https://github.com/abhikuriyal-pixel/docfoo-cli/releases/latest) · [Report an issue](https://github.com/abhikuriyal-pixel/docfoo-cli/issues)

## Install

Download and extract the archive for your platform:

| Platform | Download |
| --- | --- |
| Windows x64 | [docfoo-cli-0.1.1-windows-x64.zip](https://github.com/abhikuriyal-pixel/docfoo-cli/releases/download/v0.1.1/docfoo-cli-0.1.1-windows-x64.zip) |
| Linux / WSL x64 | [docfoo-cli-0.1.1-linux-x64.tar.gz](https://github.com/abhikuriyal-pixel/docfoo-cli/releases/download/v0.1.1/docfoo-cli-0.1.1-linux-x64.tar.gz) |

Keep `docfoo` and `docfoo-agent` in the same folder and add it to your PATH. Linux requires glibc 2.34 or newer; Ubuntu 22.04 is supported. Windows downloads are not code-signed yet.

You can also use the [Windows](https://github.com/abhikuriyal-pixel/docfoo-cli/blob/main/install.ps1) or [Linux](https://github.com/abhikuriyal-pixel/docfoo-cli/blob/main/install.sh) installer to download DocFoo and set up scanning dependencies.

### Check your download

Each archive has a `.sha256` file on the [release page](https://github.com/abhikuriyal-pixel/docfoo-cli/releases/latest). On Windows, compare its contents with:

```powershell
Get-FileHash .\docfoo-cli-0.1.1-windows-x64.zip -Algorithm SHA256
```

On Linux, download both files to the same folder and run:

```bash
sha256sum -c docfoo-cli-0.1.1-linux-x64.tar.gz.sha256
```

## Get started

Set up scanning, discover providers, and choose models:

```bash
docfoo setup
docfoo providers
docfoo auth --set PROVIDER          # API key; subscriptions use auth --login PROVIDER
docfoo models --configured --vision
docfoo model --set scan provider/vision-model
docfoo model --set kg provider/answer-model
```

Use `docfoo models --provider PROVIDER` or `--search TEXT` to browse model keys,
image-input/reasoning support and token limits. `--vision` means image input,
not native PDF support or image generation. Catalog entries do not guarantee
account access. Existing `docfoo model --list` commands still work.

Replace `PROVIDER` and the model names with entries from the list. Then scan a document, index it, and ask a question:

```bash
docfoo scan paper.pdf --output papers
docfoo kg --index --scope papers
docfoo kg --query "What are the key findings?" --scope papers
```

Your library lives in `~/.docfoo` (`%USERPROFILE%\.docfoo` on Windows). Use `--workspace DIR` to choose another folder.

Run `docfoo --help` to see the commands, or `docfoo <command> --help` for options. `docfoo doctor` checks your setup, and `docfoo update --check` checks for updates. Add `--json` when using DocFoo in a script.

## Browser login and separate accounts

ChatGPT Plus/Pro subscriptions use `openai-codex`, not an OpenAI API key:

```bash
docfoo --profile first auth --login openai-codex
docfoo --profile second auth --login openai-codex --no-browser
docfoo --profile first models --provider openai-codex
```

Complete the second login in another browser profile or private window. The login
URL and device codes are sensitive; do not share them. Browser login needs an
interactive terminal; API-key automation can use `auth --set PROVIDER --key-stdin`.

Choose `--profile first` in one terminal and `--profile second` in another, or set
terminal-local `DOCFOO_PROFILE`:

```powershell
$env:DOCFOO_PROFILE = 'first'  # PowerShell; use 'second' in the other terminal
```

Bash/zsh: `export DOCFOO_PROFILE=first`. Credentials and provider catalogs are
separate, while documents and saved model choices stay shared. Named profiles do
not copy existing credentials or custom model configuration. No profile means the
existing default login is preserved. Use `auth --status` to inspect the selected
profile without showing secrets.

Separate scans and indexes of different graph scopes can run concurrently in one
workspace. The same graph cannot be indexed twice at once, and backup/restore and
replacement operations stay exclusive. Wait for a document's scan to finish before
indexing its scope. The desktop app does not honor CLI locks; close it before
concurrent writers or destructive workspace operations.

Upgrade both `docfoo` and `docfoo-agent` together, keeping the complete archive's
files. Windows `docfoo update` stages replacements; close both processes before
replacing the installed pair.

## Your data

Your library stays on your computer. Scanning, indexing, and questions send content to the model providers you configure, and their usage charges apply. Backups contain your documents and chats, so keep them private.

For help, see [Support](SUPPORT.md). To report a vulnerability, see [Security](SECURITY.md).

## License

[MIT](LICENSE). Third-party components keep their own licenses.

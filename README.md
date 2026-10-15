# DocFoo CLI

Scan PDFs and images into a local Markdown library, then ask questions with citations back to your documents.

DocFoo extracts text, tables, and figures, organizes documents into collections, and builds knowledge graphs for searching across them. You can also browse your library, read notes, and make or restore backups from the terminal. Commands support JSON output for scripts and other applications.

[Download](https://github.com/abhikuriyal-pixel/docfoo-cli/releases/latest) · [Report an issue](https://github.com/abhikuriyal-pixel/docfoo-cli/issues)

## Install

Download and extract the archive for your platform:

| Platform | Download |
| --- | --- |
| Windows x64 | [docfoo-cli-0.1.0-windows-x64.zip](https://github.com/abhikuriyal-pixel/docfoo-cli/releases/download/v0.1.0/docfoo-cli-0.1.0-windows-x64.zip) |
| Linux / WSL x64 | [docfoo-cli-0.1.0-linux-x64.tar.gz](https://github.com/abhikuriyal-pixel/docfoo-cli/releases/download/v0.1.0/docfoo-cli-0.1.0-linux-x64.tar.gz) |

Keep `docfoo` and `docfoo-agent` in the same folder and add it to your PATH. Linux requires glibc 2.34 or newer; Ubuntu 22.04 is supported. Windows downloads are not code-signed yet.

You can also use the [Windows](https://github.com/abhikuriyal-pixel/docfoo-cli/blob/main/install.ps1) or [Linux](https://github.com/abhikuriyal-pixel/docfoo-cli/blob/main/install.sh) installer to download DocFoo and set up scanning dependencies.

### Check your download

Each archive has a `.sha256` file on the [release page](https://github.com/abhikuriyal-pixel/docfoo-cli/releases/latest). On Windows, compare its contents with:

```powershell
Get-FileHash .\docfoo-cli-0.1.0-windows-x64.zip -Algorithm SHA256
```

On Linux, download both files to the same folder and run:

```bash
sha256sum -c docfoo-cli-0.1.0-linux-x64.tar.gz.sha256
```

## Get started

Set up scanning, add your provider's API key, and choose models from `docfoo model --list`:

```bash
docfoo setup
docfoo auth --set PROVIDER
docfoo model --list
docfoo model --set scan provider/vision-model
docfoo model --set kg provider/answer-model
```

Replace `PROVIDER` and the model names with entries from the list. Then scan a document, index it, and ask a question:

```bash
docfoo scan paper.pdf --output papers
docfoo kg --index --scope papers
docfoo kg --query "What are the key findings?" --scope papers
```

Your library lives in `~/.docfoo` (`%USERPROFILE%\.docfoo` on Windows). Use `--workspace DIR` to choose another folder.

Run `docfoo --help` to see the commands, or `docfoo <command> --help` for options. `docfoo doctor` checks your setup, and `docfoo update --check` checks for updates. Add `--json` when using DocFoo in a script.

## Your data

Your library stays on your computer. Scanning, indexing, and questions send content to the model providers you configure, and their usage charges apply. Backups contain your documents and chats, so keep them private.

For help, see [Support](SUPPORT.md). To report a vulnerability, see [Security](SECURITY.md).

## License

[MIT](LICENSE). Third-party components keep their own licenses.

# DocFoo CLI

**From documents to grounded answers — in your terminal or application.**

DocFoo CLI scans PDFs and images into a local Markdown library, builds knowledge graphs, and answers questions with citations and figures.

[Download](https://github.com/abhikuriyal-pixel/docfoo-cli/releases) · [Report an issue](https://github.com/abhikuriyal-pixel/docfoo-cli/issues) · [Support](SUPPORT.md)

## Features

- PDF and image scanning with text, tables, and figures.
- Knowledge-graph indexing and queries scoped to a collection or the whole library.
- Answers with citations, figures, and graph-replay snapshots.
- Resource, note, collection, backup, and restore commands.
- Model configuration, credential management, and local health checks.
- Structured JSON output, shell completions, and application integrations.

## Install

Download the archive and checksum for your platform from [Releases](https://github.com/abhikuriyal-pixel/docfoo-cli/releases):

| Platform | Archive |
| --- | --- |
| Windows x64 | `docfoo-cli-0.1.0-windows-x64.zip` |
| Linux / WSL x64 | `docfoo-cli-0.1.0-linux-x64.tar.gz` |

Keep `docfoo` and `docfoo-agent` together. Windows executables are unsigned. Linux requires glibc 2.34 or newer, including Ubuntu 22.04. macOS and ARM packages are not available.

Verify the download using its `.sha256` file. On Windows:

```powershell
Get-FileHash .\docfoo-cli-0.1.0-windows-x64.zip -Algorithm SHA256
```

On Linux:

```bash
sha256sum -c docfoo-cli-0.1.0-linux-x64.tar.gz.sha256
tar -xzf docfoo-cli-0.1.0-linux-x64.tar.gz
```

The [Windows](install.ps1) and [Linux](install.sh) installers can install the binaries and set up scanning dependencies.

## Getting started

```bash
docfoo version
docfoo doctor
docfoo setup
docfoo auth --set PROVIDER
docfoo model --list
docfoo model --set scan provider/vision-model
docfoo model --set kg provider/answer-model
docfoo scan paper.pdf --output papers
docfoo kg --index --scope papers
docfoo kg --query "Summarize the key findings" --scope papers
```

Replace the provider and model placeholders with available catalogue entries. `docfoo setup` downloads the scanning dependencies. The default workspace is `~/.docfoo` (`%USERPROFILE%\.docfoo` on Windows).

Run `docfoo <command> --help` for options. Add `--json` for scripts and application integrations. Check for updates with `docfoo update --check`.

## Privacy

The document library is local, but configured model providers receive document and query content and may charge for usage. Keep credentials, document libraries, and backup archives private. Do not include them in issue reports.

## License

[MIT](LICENSE). Bundled third-party components retain their respective licenses. See [SECURITY.md](SECURITY.md) for vulnerability reporting.

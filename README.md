# EXDViewer
<img align="right" src="https://github.com/WorkingRobot/EXDViewer/blob/main/viewer/assets/icon.png?raw=true" width="20%">

[![Native Build](https://img.shields.io/github/actions/workflow/status/WorkingRobot/EXDViewer/build-native.yml?style=for-the-badge&label=Native%20Build
)](https://github.com/WorkingRobot/EXDViewer/releases)
[![Web Build](https://img.shields.io/github/actions/workflow/status/WorkingRobot/EXDViewer/build-web.yml?style=for-the-badge&label=Web%20Build
)](https://github.com/WorkingRobot/EXDViewer/pkgs/container/exdviewer-web)
[![License](https://img.shields.io/github/license/WorkingRobot/EXDViewer?style=for-the-badge)](/LICENSE)
[![FFXIV Version](https://img.shields.io/badge/dynamic/json?url=https%3A%2F%2Fexd.camora.dev%2Fapi%2Fversions&query=latest&style=for-the-badge&label=Latest%20XIV%20Version
)](https://thaliak.xiv.dev/repository/4e9a232b)


EXDViewer is a modern, fast, and user-friendly tool for exploring [Excel files](https://xiv.dev/game-data/file-formats/excel) from Final Fantasy XIV Online. Excel files are structured data tables that store various in-game information, such as item stats, NPC data, and more.

## Features

- **Web and Native:** Instantly use the web version at [exd.camora.dev](https://exd.camora.dev) or download [a native build](https://github.com/WorkingRobot/EXDViewer/releases).
- **Easy Deployment:** Host your own web instance via Docker.
- **Performance:** Efficiently handles all sheets, even huge ones like `Item`, `Action`, or `Quest`.
- **EXDSchema Support:** Provides tight integration with [EXDSchema](https://github.com/xivdev/EXDSchema) for enhanced data exploration and dynamic in-viewer schema editing.
- **Advanced Filtering:** Supports simple, fuzzy, and complex filtering to quickly find specific data.

## Quick Start

### Online

Visit [exd.camora.dev](https://exd.camora.dev) to use the latest version, directly in your browser. Supports local game installs and schema files ([Chromium-based browsers only](https://developer.mozilla.org/en-US/docs/Web/API/Window/showDirectoryPicker#browser_compatibility)).

### Locally

Find pre-built binaries for your platform on the [Releases page](https://github.com/WorkingRobot/EXDViewer/releases).

### Self-Host with Docker

Deploy the website yourself with Docker:

```bash
docker pull ghcr.io/workingrobot/exdviewer-web:main
docker run -p 8080:80 ghcr.io/workingrobot/exdviewer-web:main
```
Then open [http://localhost:8080](http://localhost:8080) in your browser. Give it a few minutes to download the latest game version, and set the API url to `http://localhost:8080/api` in the settings.

## What Are EXD Files?

Inside SqPack, category 0A (0a0000.win32... files) consists of Excel sheets serialized into a proprietary binary format read by the game. Excel files (of which .exd files contain the actual data) are a core part of Final Fantasy XIV's data storage, containing tabular information such as quests, items, and more. They're often used by the FFXIV community for datamining and developing community tools. Programmatic access to these files is typically done through via [Lumina](https://github.com/NotAdam/Lumina) (C#), [ironworks](https://github.com/ackwell/ironworks) (Rust), or [XIVAPI](https://xivapi.com/) (REST API).

More info is available [here](https://xiv.dev/game-data/file-formats/excel).

## What is EXDSchema?

FFXIV's internal development cycle generates header files for each sheet, which are then compiled into the game, thus, all structure information is lost on the client side when the game is compiled. This repository is an attempt to consolidate efforts into a language agnostic schema, easily parsed into any language that wishes to consume it, that accurately describes the structure of the EXH files as they are provided to the client.

More info is available [here](https://github.com/xivdev/EXDSchema?tab=readme-ov-file#exdschema).

## Building from Source

1. Clone the repository:
    ```bash
    git clone https://github.com/WorkingRobot/EXDViewer.git
    cd EXDViewer
    ```

### Native

2. Build the project:
    ```bash
    cargo build --bin exdviewer --release
    ```

### Web

2. Install trunk:
    ```bash
    cargo install --locked trunk
    ```
    or follow the [instructions](https://trunkrs.dev/guide/getting-started/installation.html).
    Make sure `trunk` is installed and available in your PATH before continuing.

3. Build the web version:
    ```bash
    cargo run --bin exdviewer-web --release
    ```

## Contributing

Contributions, bug reports, and feature requests are welcome! Please open an [issue](https://github.com/WorkingRobot/EXDViewer/issues) or a [pull request](https://github.com/WorkingRobot/EXDViewer/pulls).

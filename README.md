# Serial-TUI

> **A blazingly fast, feature-rich terminal user interface for serial port communication.** Serial-TUI brings the power and elegance of modern TUI design to embedded development and serial debugging. Navigate with vim-style keybindings, copy output with precision, and manage multiple devices seamlessly—all from the comfort of your terminal.

[![Rust](https://img.shields.io/badge/rust-%23000000.svg?style=flat&logo=rust&logoColor=white)](https://www.rust-lang.org/)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

## 🚀 Features

### 🔌 Device Management
- **Auto-detection** of serial ports (USB, PCI, Bluetooth)
- **Smart filtering** excludes system devices and pseudo-terminals
- **Device information display** with product names for USB devices
- **Dynamic refresh** to discover newly connected devices

### ⚡ Serial Communication
- **8 pre-configured baud rates**: 9600, 19200, 38400, 57600, 115200, 230400, 460800, 921600
- **Custom baud rate support** for any value
- **Real-time bidirectional communication** with automatic line termination
- **Multi-threaded architecture** for responsive UI and reliable I/O
- **50KB rolling buffer** with automatic trimming

### 📜 Command History
- **Persistent history** stored at `~/.serial-tui_history`
- **Smart deduplication** moves repeated commands to the end
- **Interactive history pane** for browsing and reusing commands
- **Direct history execution** with a single keypress

### 🎯 Visual Mode & Clipboard
- **Vim-style navigation** with `h/j/k/l` keys
- **Visual selection mode** for precise text copying
- **Cross-platform clipboard** integration
- **Line-by-line or range selection** with visual feedback
- **Auto-scrolling** keeps cursor visible during navigation

### 🎨 Modern TUI Design
- **Multi-pane interface** with Tab-based navigation
- **Color-coded status indicators** (green = connected, red = disconnected)
- **Context-sensitive help** changes based on active pane
- **Responsive 60 FPS UI** with smooth scrolling
- **About dialog** with project information

## 📦 Installation

### From Source

```bash
# Clone the repository
git clone https://github.com/yourusername/serial-tui.git
cd serial-tui

# Build and install
cargo build --release
sudo cp target/release/serial-tui /usr/local/bin/
```

### Using Cargo

```bash
cargo install serial-tui
```

## 🎮 Usage

### Quick Start

```bash
# Launch the application
serial-tui
```

### Interface Layout

```
┌─────────────────────────────────────────────────────────────┐
│ [DEVICES]              Serial-TUI              [Connected]  │
├─────────────┬─────────────────────────────────────────────────┤
│             │                                               │
│  Devices    │              RX/TX Output                     │
│  --------   │                                               │
│  > USB0     │  > Hello                                      │
│    USB1     │  World!                                       │
│             │  > status                                     │
│             │  OK                                           │
│             │                                               │
├─────────────┼─────────────────────────────────────────────────┤
│  Baud Rate  │              Command History                  │
│  ---------  │                                               │
│  > 115200   │  1. help                                      │
│             │  2. status                                    │
├─────────────┴─────────────────────────────────────────────────┤
│ Input: _                                                    │
├─────────────────────────────────────────────────────────────┤
│ Help: Tab/Shift+Tab=switch | Enter=send | ?=help | q=quit  │
└─────────────────────────────────────────────────────────────┘
```

## ⌨️ Keyboard Shortcuts

### Global Commands
| Key | Action |
|-----|--------|
| `Tab` | Switch to next pane |
| `Shift+Tab` | Switch to previous pane |
| `q` | Quit (except in Input pane) |
| `?` | Show about dialog |
| `v` | Enter visual mode |
| `r` | Refresh device list |

### Devices Pane
| Key | Action |
|-----|--------|
| `j` / `↓` | Move selection down |
| `k` / `↑` | Move selection up |
| `c` | Connect to selected device |
| `d` | Disconnect from device |
| `b` | Switch to Baud pane |

### Baud Rate Pane
| Key | Action |
|-----|--------|
| `j` / `↓` | Select next baud rate |
| `k` / `↑` | Select previous baud rate |
| `c` | Connect with selected baud |
| `d` | Disconnect |
| `Enter` | Confirm baud selection |
| `0-9` | Enter custom baud rate |
| `Backspace` | Delete digit |

### History Pane
| Key | Action |
|-----|--------|
| `j` / `↓` | Scroll down history |
| `k` / `↑` | Scroll up history |
| `l` | Load command into input |
| `Enter` | Send command immediately |

### Input Pane
| Key | Action |
|-----|--------|
| Type | Enter command text |
| `Backspace` | Delete character |
| `Enter` | Send command |

### Visual Mode
| Key | Action |
|-----|--------|
| `h` | Move cursor left |
| `j` | Move cursor down |
| `k` | Move cursor up |
| `l` | Move cursor right |
| `Space` | Start text selection |
| `Enter` | Copy selection/line |
| `Esc` / `v` | Exit visual mode |

## 🛠️ Configuration

### History File

Command history is automatically saved to:
```
~/.serial-tui_history
```

### Supported Device Types

Serial-TUI automatically detects and filters:
- ✅ **USB serial ports** (with product information)
- ✅ **PCI serial ports**
- ✅ **Bluetooth serial ports**
- ❌ System devices (`/dev/tty`, `/dev/console`, etc.)
- ❌ Pseudo-terminals (`/dev/pts/*`, `/dev/ptmx`)

### Serial Port Settings

- **Data bits**: 8
- **Stop bits**: 1
- **Parity**: None
- **Flow control**: None
- **Line termination**: `\r\n` (automatically appended)
- **Read timeout**: 50ms

## 🏗️ Architecture

### Technology Stack

- **[Ratatui](https://github.com/ratatui-org/ratatui)** (v0.28) - Terminal UI framework
- **[CrossTerm](https://github.com/crossterm-rs/crossterm)** (v0.28) - Terminal control
- **[SerialPort](https://gitlab.com/susurrus/serialport-rs)** (v4.6) - Serial communication
- **[Tokio](https://tokio.rs/)** (v1) - Async runtime
- **[Arboard](https://github.com/1Password/arboard)** (v3) - Clipboard access
- **[Anyhow](https://github.com/dtolnay/anyhow)** (v1) - Error handling

### Design Philosophy

- **Multi-threaded I/O**: Separate threads for reading and writing to prevent UI blocking
- **Non-blocking UI**: 16ms refresh rate (~60 FPS) for smooth interaction
- **Memory efficient**: Rolling buffer with automatic trimming
- **Fail-safe**: Graceful error handling and connection recovery

## 🤝 Contributing

Contributions are welcome! Please feel free to submit a Pull Request. For major changes, please open an issue first to discuss what you would like to change.

### Development Setup

```bash
# Clone the repository
git clone https://github.com/yourusername/serial-tui.git
cd serial-tui

# Build in debug mode
cargo build

# Run tests
cargo test

# Run with debug output
RUST_LOG=debug cargo run
```

## 📝 License

This project is licensed under the MIT License - see the [LICENSE](LICENSE) file for details.

## 🙏 Acknowledgments

- Built with [Ratatui](https://github.com/ratatui-org/ratatui) - The best Rust TUI framework
- Inspired by classic serial terminal tools with modern enhancements
- Thanks to all contributors and the Rust community

## 📧 Contact

- **Issues**: [GitHub Issues](https://github.com/yourusername/serial-tui/issues)
- **Discussions**: [GitHub Discussions](https://github.com/yourusername/serial-tui/discussions)

---

<div align="center">
Made with ❤️ and 🦀 Rust
</div>

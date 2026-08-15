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

#### Connection Dialog (Initial Screen)
The app starts with a vertically stacked connection configuration dialog:

```
┌─────────────────────────────────────────────────────────────┐
│          Serial-TUI - Connection Configuration              │
├─────────────────────────────────────────────────────────────┤
│ Devices (Tab cycles, j/k or arrows, c=connect, r=refresh)  │
│  > /dev/ttyUSB0 (USB Serial Device)                        │
│    /dev/ttyUSB1 (USB Serial Device)                        │
│    /dev/ttyACM0 (Arduino Mega)                             │
├─────────────────────────────────────────────────────────────┤
│ Baud Rate                                                   │
│    9600                                                     │
│  > 115200                                                   │
│    230400                                                   │
├─────────────────────────────────────────────────────────────┤
│ Data Bits: > 8 bits                                         │
├─────────────────────────────────────────────────────────────┤
│ Stop Bits: > 1                                              │
├─────────────────────────────────────────────────────────────┤
│ Parity: > None                                              │
├─────────────────────────────────────────────────────────────┤
│ Press 'c' to connect | 'r' to refresh | 'q' to quit        │
└─────────────────────────────────────────────────────────────┘
```

#### Connected Interface
After connecting, the interface switches to vertically stacked operational view:

```
┌─────────────────────────────────────────────────────────────┐
│ Serial Data (Tab to History, v=visual mode)                │
│  > Hello                                                    │
│  World!                                                     │
│  > status                                                   │
│  OK                                                         │
│  > help                                                     │
│  Available commands: help, status, reset                    │
├─────────────────────────────────────────────────────────────┤
│ Command History (Tab to Input, j/k, Enter to send)         │
│  3. reset                                                   │
│  2. status                                                  │
│  1. help                                                    │
├─────────────────────────────────────────────────────────────┤
│ Input: > _                                                  │
├─────────────────────────────────────────────────────────────┤
│ Input captures typing; Esc leaves input focus              │
└─────────────────────────────────────────────────────────────┘
```

**Key UI Features:**
- **Vertical navigation**: Tab moves down through panes, Shift+Tab moves up
- **Connection dialog first**: Configure all settings before connecting
- **Compact connected view**: Device and baud stay in the status bar, not a separate pane
- **Focused workflow**: Each screen optimized for its task

## ⌨️ Keyboard Shortcuts

### Global Commands
| Key | Action |
|-----|--------|
| `Tab` | Move to next pane (down) |
| `Shift+Tab` | Move to previous pane (up) |
| `?` | Show about dialog |
| `v` | Enter visual mode (when connected) |

### Connection Dialog
| Key | Action |
|-----|--------|
| `Tab` / `Shift+Tab` | Cycle through: Device → Baud → Data Bits → Stop Bits → Parity |
| `j` / `k` / `↓` / `↑` | Navigate within current setting |
| `c` | Connect with selected settings |
| `r` | Refresh device list |
| `q` | Quit application |
| `0-9` | Enter custom baud rate (when on Custom option) |
| `Backspace` | Delete digit (custom baud) |

### Connected Interface
| Key | Action |
|-----|--------|
| `Tab` / `Shift+Tab` | Cycle through: Serial Data → History → Input |
| `d` | Disconnect (returns to connection dialog) |
| `q` | Quit application when not in Input |
| `v` | Enter visual mode (in Serial Data pane) |

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
| `Tab` / `Shift+Tab` | Switch panes while input is focused |
| `Esc` | Leave input focus |

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

### Configuration Directory

Command history and configuration files are stored in:
```
~/.config/serial-tui/history
```

The directory is automatically created on first run if it doesn't exist.

### Supported Device Types

Serial-TUI automatically detects and filters:
- ✅ **USB serial ports** (with product information)
- ✅ **PCI serial ports**
- ✅ **Bluetooth serial ports**
- ❌ System devices (`/dev/tty`, `/dev/console`, etc.)
- ❌ Pseudo-terminals (`/dev/pts/*`, `/dev/ptmx`)

### Serial Port Settings

Configurable in the connection dialog:
- **Baud rates**: 9600, 19200, 38400, 57600, 115200, 230400, 460800, 921600, or custom
- **Data bits**: 5, 6, 7, 8 (default: 8)
- **Stop bits**: 1, 1.5, 2 (default: 1)
- **Parity**: None, Odd, Even (default: None)

Fixed settings:
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

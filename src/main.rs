use anyhow::Result;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    prelude::Stylize,
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph},
    Frame, Terminal,
};
use serialport::SerialPortType;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

const BAUD_RATES: &[u32] = &[9600, 19200, 38400, 57600, 115200, 230400, 460800, 921600];
const DATA_BITS: &[u8] = &[5, 6, 7, 8];
const STOP_BITS_OPTIONS: &[&str] = &["1", "1.5", "2"];
const PARITY_OPTIONS: &[&str] = &["None", "Odd", "Even"];

fn get_history_path() -> std::path::PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    let config_dir = std::path::PathBuf::from(home).join(".config/serial-tui");
    // Create directory if it doesn't exist
    let _ = std::fs::create_dir_all(&config_dir);
    config_dir.join("history")
}

fn load_history() -> Vec<String> {
    let path = get_history_path();
    if let Ok(content) = std::fs::read_to_string(&path) {
        content.lines().map(|s| s.to_string()).collect()
    } else {
        Vec::new()
    }
}

fn save_history(history: &[String]) {
    let path = get_history_path();
    let content = history.join("\n");
    let _ = std::fs::write(&path, content);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AppState {
    ConnectionDialog,
    Connected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConnectionPane {
    Device,
    BaudRate,
    DataBits,
    StopBits,
    Parity,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConnectedPane {
    SerialData,
    History,
    Input,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VisualMode {
    Normal,
    Visual,
    Selecting,
}

struct App {
    app_state: AppState,
    devices: Vec<String>,
    device_paths: Vec<String>,
    selected_device: Option<usize>,
    selected_baud: usize,
    selected_data_bits: usize,
    selected_stop_bits: usize,
    selected_parity: usize,
    connected_device: String,
    connected_baud: u32,
    rx_buffer: String,
    tx_input: String,
    tx_sender: Option<Sender<String>>,
    rx_receiver: Option<Receiver<String>>,
    cmd_history: Vec<String>,
    history_index: Option<usize>,
    status_msg: String,
    connection_pane: ConnectionPane,
    connected_pane: ConnectedPane,
    device_scroll: usize,
    baud_scroll: usize,
    data_bits_scroll: usize,
    history_scroll: usize,
    history_selected: usize,
    visual_mode: VisualMode,
    cursor_line: usize,
    cursor_col: usize,
    selection_start_line: usize,
    selection_start_col: usize,
    should_stop: Arc<AtomicBool>,
    rx_scroll: usize,
    clipboard: Option<arboard::Clipboard>,
    show_about: bool,
    custom_baud: String,
    custom_baud_value: Option<u32>,
}

impl App {
    fn new() -> Self {
        let cmd_history = load_history();
        let selected_baud = 4; // 115200 default
        let selected_data_bits = 3; // 8 bits default
        Self {
            app_state: AppState::ConnectionDialog,
            devices: Vec::new(),
            device_paths: Vec::new(),
            selected_device: None,
            selected_baud,
            selected_data_bits,
            selected_stop_bits: 0, // 1 stop bit default
            selected_parity: 0,    // None default
            connected_device: String::new(),
            connected_baud: 0,
            rx_buffer: String::new(),
            tx_input: String::new(),
            tx_sender: None,
            rx_receiver: None,
            cmd_history,
            history_index: None,
            status_msg: String::new(),
            connection_pane: ConnectionPane::Device,
            connected_pane: ConnectedPane::Input,
            device_scroll: 0,
            baud_scroll: selected_baud.saturating_sub(3),
            data_bits_scroll: selected_data_bits.saturating_sub(2),
            history_scroll: 0,
            history_selected: 0,
            visual_mode: VisualMode::Normal,
            cursor_line: 0,
            cursor_col: 0,
            selection_start_line: 0,
            selection_start_col: 0,
            should_stop: Arc::new(AtomicBool::new(false)),
            rx_scroll: 0,
            clipboard: arboard::Clipboard::new().ok(),
            show_about: false,
            custom_baud: String::new(),
            custom_baud_value: None,
        }
    }

    fn refresh_devices(&mut self) -> Result<()> {
        let ports = serialport::available_ports()?;
        self.devices.clear();
        self.device_paths.clear();
        for port in ports {
            let path = port.port_name.clone();

            // Filter out system tty devices that aren't useful
            if path == "/dev/tty"
                || path == "/dev/console"
                || path == "/dev/ptmx"
                || path.starts_with("/dev/pts/")
                || path.starts_with("/dev/ttyS")
            // Old-style serial ports usually not connected
            {
                continue;
            }

            let name = match port.port_type {
                SerialPortType::UsbPort(info) => {
                    format!(
                        "{} ({})",
                        path,
                        info.product.as_ref().unwrap_or(&"USB".to_string())
                    )
                }
                SerialPortType::PciPort => format!("{} (PCI)", path),
                SerialPortType::BluetoothPort => format!("{} (Bluetooth)", path),
                SerialPortType::Unknown => path.clone(),
            };
            self.device_paths.push(path);
            self.devices.push(name);
        }
        if self.devices.is_empty() {
            self.status_msg = "No devices found".to_string();
        } else {
            self.status_msg = format!("Found {} device(s)", self.devices.len());
        }
        Ok(())
    }

    fn connect(&mut self) {
        if self.selected_device.is_none() {
            self.status_msg = "No device selected".to_string();
            return;
        }

        let device_idx = self.selected_device.unwrap();
        let device_path = match self.device_paths.get(device_idx).cloned() {
            Some(p) => p,
            None => {
                self.status_msg = "Device not found".to_string();
                return;
            }
        };

        let baud = BAUD_RATES[self.selected_baud];

        self.should_stop.store(false, Ordering::SeqCst);
        let should_stop = self.should_stop.clone();

        let (tx_cmd, rx_cmd) = mpsc::channel::<String>();
        let (tx_data, rx_data) = mpsc::channel::<String>();

        self.tx_sender = Some(tx_cmd);
        self.rx_receiver = Some(rx_data);

        let port = match serialport::new(&device_path, baud)
            .timeout(Duration::from_millis(50))
            .open()
        {
            Ok(p) => p,
            Err(e) => {
                self.status_msg = format!("Failed to open {}: {}", device_path, e);
                return;
            }
        };

        let port_read = match port.try_clone() {
            Ok(p) => p,
            Err(e) => {
                self.status_msg = format!("Failed to clone port: {}", e);
                return;
            }
        };
        let port_write = port;

        let tx_data_clone = tx_data.clone();
        let should_stop_read = should_stop.clone();
        thread::spawn(move || {
            let mut port = port_read;
            let mut buf = [0u8; 2048];
            loop {
                if should_stop_read.load(Ordering::SeqCst) {
                    break;
                }
                match port.read(&mut buf) {
                    Ok(n) if n > 0 => {
                        let data = String::from_utf8_lossy(&buf[..n]).to_string();
                        let _ = tx_data_clone.send(data);
                    }
                    Ok(_) => {}
                    Err(e) => {
                        if e.kind() != std::io::ErrorKind::TimedOut {
                            break;
                        }
                    }
                }
                thread::sleep(Duration::from_millis(5));
            }
        });

        let should_stop_write = should_stop.clone();
        thread::spawn(move || {
            let mut port = port_write;
            while let Ok(cmd) = rx_cmd.recv() {
                if should_stop_write.load(Ordering::SeqCst) {
                    break;
                }
                let _ = port.write_all(cmd.as_bytes());
                let _ = port.write_all(b"\r\n");
                let _ = port.flush();
            }
        });

        self.app_state = AppState::Connected;
        self.connected_device = device_path.clone();
        self.connected_baud = baud;
        self.status_msg = format!("Connected to {} at {} baud", device_path, baud);
        self.connected_pane = ConnectedPane::Input;
    }

    fn disconnect(&mut self) {
        self.should_stop.store(true, Ordering::SeqCst);
        self.tx_sender = None;
        self.rx_receiver = None;
        self.app_state = AppState::ConnectionDialog;
        self.connected_device.clear();
        self.connected_baud = 0;
        self.rx_buffer.clear();
        self.tx_input.clear();
        self.status_msg = "Disconnected".to_string();
    }

    fn next_pane(&mut self) {
        match self.app_state {
            AppState::ConnectionDialog => {
                self.connection_pane = match self.connection_pane {
                    ConnectionPane::Device => ConnectionPane::BaudRate,
                    ConnectionPane::BaudRate => ConnectionPane::DataBits,
                    ConnectionPane::DataBits => ConnectionPane::StopBits,
                    ConnectionPane::StopBits => ConnectionPane::Parity,
                    ConnectionPane::Parity => ConnectionPane::Device,
                };
            }
            AppState::Connected => {
                self.connected_pane = match self.connected_pane {
                    ConnectedPane::SerialData => ConnectedPane::History,
                    ConnectedPane::History => ConnectedPane::Input,
                    ConnectedPane::Input => ConnectedPane::SerialData,
                };
            }
        }
    }

    fn prev_pane(&mut self) {
        match self.app_state {
            AppState::ConnectionDialog => {
                self.connection_pane = match self.connection_pane {
                    ConnectionPane::Device => ConnectionPane::Parity,
                    ConnectionPane::BaudRate => ConnectionPane::Device,
                    ConnectionPane::DataBits => ConnectionPane::BaudRate,
                    ConnectionPane::StopBits => ConnectionPane::DataBits,
                    ConnectionPane::Parity => ConnectionPane::StopBits,
                };
            }
            AppState::Connected => {
                self.connected_pane = match self.connected_pane {
                    ConnectedPane::SerialData => ConnectedPane::Input,
                    ConnectedPane::History => ConnectedPane::SerialData,
                    ConnectedPane::Input => ConnectedPane::History,
                };
            }
        }
    }

    fn send_command(&mut self) {
        if let Some(tx) = &self.tx_sender {
            let cmd = self.tx_input.clone();
            let _ = tx.send(cmd);
            if !self.tx_input.is_empty() {
                self.rx_buffer.push_str(&format!("> {}\n", self.tx_input));
            } else {
                self.rx_buffer.push_str("> \n");
            }
        }

        // Only add non-empty commands to history
        if !self.tx_input.is_empty() {
            // Remove command from history if it already exists, then add to end
            let cmd = self.tx_input.clone();
            self.cmd_history.retain(|c| c != &cmd);
            self.cmd_history.push(cmd);
            save_history(&self.cmd_history);
        }

        self.history_index = None;
        self.history_scroll = 0;
        self.tx_input.clear();
    }

    fn send_from_history(&mut self) {
        if !self.cmd_history.is_empty() {
            let idx = self.cmd_history.len() - 1 - self.history_selected;
            if let Some(cmd) = self.cmd_history.get(idx).cloned() {
                if let Some(tx) = &self.tx_sender {
                    let _ = tx.send(cmd.clone());
                    self.rx_buffer.push_str(&format!("> {}\n", cmd));
                }

                // Move this command to the end (most recent) in history
                self.cmd_history.retain(|c| c != &cmd);
                self.cmd_history.push(cmd);
                save_history(&self.cmd_history);

                self.history_scroll = 0;
            }
        }
    }

    fn load_history_to_input(&mut self) {
        // Use the selected history item
        if !self.cmd_history.is_empty() {
            let idx = self.cmd_history.len() - 1 - self.history_selected;
            if let Some(cmd) = self.cmd_history.get(idx) {
                self.tx_input = cmd.clone();
            }
        }
    }

    fn enter_visual_mode(&mut self) {
        if self.rx_buffer.is_empty() {
            return;
        }
        self.visual_mode = VisualMode::Visual;

        // Start cursor at the bottom (last line)
        let lines = self.get_rx_lines();
        let last_line = lines.len().saturating_sub(1);
        self.cursor_line = last_line;
        self.cursor_col = 0;
        self.selection_start_line = last_line;
        self.selection_start_col = 0;

        // Scroll to show the bottom
        self.rx_scroll = last_line;
    }

    fn exit_visual_mode(&mut self) {
        self.visual_mode = VisualMode::Normal;
    }

    fn start_selection(&mut self) {
        self.visual_mode = VisualMode::Selecting;
        self.selection_start_line = self.cursor_line;
        self.selection_start_col = self.cursor_col;
    }

    fn get_rx_lines(&self) -> Vec<String> {
        self.rx_buffer.lines().map(|s| s.to_string()).collect()
    }

    fn get_selection_text(&self) -> String {
        let lines = self.get_rx_lines();
        if lines.is_empty() {
            return String::new();
        }

        let start_line = self.selection_start_line.min(self.cursor_line);
        let end_line = self.selection_start_line.max(self.cursor_line);
        let start_col = if start_line == end_line {
            self.selection_start_col.min(self.cursor_col)
        } else if self.selection_start_line < self.cursor_line {
            self.selection_start_col
        } else {
            self.cursor_col
        };
        let end_col = if start_line == end_line {
            self.selection_start_col.max(self.cursor_col)
        } else if self.selection_start_line < self.cursor_line {
            self.cursor_col
        } else {
            self.selection_start_col
        };

        let mut result = String::new();
        for (i, line) in lines.iter().enumerate() {
            if i < start_line || i > end_line {
                continue;
            }
            if i == start_line && i == end_line {
                // Include character at cursor position (end_col + 1)
                let end_pos = (end_col + 1).min(line.len());
                result.push_str(&line[start_col..end_pos]);
            } else if i == start_line {
                result.push_str(&line[start_col..]);
                result.push('\n');
            } else if i == end_line {
                // Include character at cursor position (end_col + 1)
                let end_pos = (end_col + 1).min(line.len());
                result.push_str(&line[..end_pos]);
            } else {
                result.push_str(line);
                result.push('\n');
            }
        }
        result
    }

    fn copy_selection_to_clipboard(&mut self) -> bool {
        let text = self.get_selection_text();
        if text.is_empty() {
            self.status_msg = "Nothing selected".to_string();
            return false;
        }

        // Try to use the persistent clipboard first
        if let Some(ref mut cb) = self.clipboard {
            match cb.set_text(text.clone()) {
                Ok(_) => {
                    self.status_msg = format!("Copied {} chars!", text.len());
                    return true;
                }
                Err(_) => {
                    // Try to reinitialize
                    self.clipboard = arboard::Clipboard::new().ok();
                }
            }
        }

        // If persistent clipboard doesn't exist or failed, try creating new one
        match arboard::Clipboard::new() {
            Ok(mut cb) => match cb.set_text(text.clone()) {
                Ok(_) => {
                    // Keep this clipboard instance alive
                    self.clipboard = Some(cb);
                    self.status_msg = format!("Copied {} chars!", text.len());
                    true
                }
                Err(e) => {
                    self.status_msg = format!("Copy failed: {}", e);
                    false
                }
            },
            Err(e) => {
                self.status_msg = format!("Clipboard unavailable: {}", e);
                false
            }
        }
    }

    fn move_cursor(&mut self, direction: &str, visible_height: usize) {
        let lines = self.get_rx_lines();
        if lines.is_empty() {
            return;
        }

        let max_line = lines.len() - 1;
        let max_col = lines.get(self.cursor_line).map(|l| l.len()).unwrap_or(0);

        match direction {
            "h" => {
                if self.cursor_col > 0 {
                    self.cursor_col -= 1;
                } else if self.cursor_line > 0 {
                    self.cursor_line -= 1;
                    self.cursor_col = lines.get(self.cursor_line).map(|l| l.len()).unwrap_or(0);
                }
            }
            "j" => {
                if self.cursor_line < max_line {
                    self.cursor_line += 1;
                    self.cursor_col = self
                        .cursor_col
                        .min(lines.get(self.cursor_line).map(|l| l.len()).unwrap_or(0));
                    if self.cursor_line > self.rx_scroll + visible_height - 1 {
                        self.rx_scroll = self.cursor_line - visible_height + 1;
                    }
                }
            }
            "k" => {
                if self.cursor_line > 0 {
                    self.cursor_line -= 1;
                    self.cursor_col = self
                        .cursor_col
                        .min(lines.get(self.cursor_line).map(|l| l.len()).unwrap_or(0));
                    if self.cursor_line < self.rx_scroll {
                        self.rx_scroll = self.cursor_line;
                    }
                }
            }
            "l" => {
                if self.cursor_col < max_col {
                    self.cursor_col += 1;
                } else if self.cursor_line < max_line {
                    self.cursor_line += 1;
                    self.cursor_col = 0;
                    if self.cursor_line > self.rx_scroll + visible_height - 1 {
                        self.rx_scroll = self.cursor_line - visible_height + 1;
                    }
                }
            }
            _ => {}
        }
    }

    fn poll_rx(&mut self) {
        if let Some(rx) = &self.rx_receiver {
            while let Ok(data) = rx.try_recv() {
                self.rx_buffer.push_str(&data);
                if self.rx_buffer.len() > 50000 {
                    self.rx_buffer = self.rx_buffer.split_off(25000);
                }
            }
        }
    }
}

fn connection_pane_title(pane: ConnectionPane) -> &'static str {
    match pane {
        ConnectionPane::Device => "Devices",
        ConnectionPane::BaudRate => "Baud Rate",
        ConnectionPane::DataBits => "Data Bits",
        ConnectionPane::StopBits => "Stop Bits",
        ConnectionPane::Parity => "Parity",
    }
}

fn connected_pane_title(pane: ConnectedPane) -> &'static str {
    match pane {
        ConnectedPane::SerialData => "Serial Data",
        ConnectedPane::History => "History",
        ConnectedPane::Input => "Input",
    }
}

fn render_list_pane(
    f: &mut Frame,
    area: Rect,
    title: &str,
    items: Vec<ListItem>,
    active: bool,
    _scroll_offset: usize, // Kept for API compatibility but not used - caller does the skipping
) {
    let list = List::new(items).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(if active {
                Style::default().fg(Color::Green)
            } else {
                Style::default().fg(Color::White)
            })
            .title(title),
    );
    f.render_widget(list, area);
}

fn render_connection_dialog(f: &mut Frame, area: Rect, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(6),
            Constraint::Length(6),
            Constraint::Length(5),
            Constraint::Length(5),
            Constraint::Length(5),
        ])
        .split(area);

    let device_items: Vec<ListItem> = if app.devices.is_empty() {
        vec![ListItem::new("No devices found")]
    } else {
        app.devices
            .iter()
            .enumerate()
            .skip(app.device_scroll)
            .map(|(i, d)| {
                let style = if Some(i) == app.selected_device {
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(ratatui::style::Modifier::BOLD)
                } else {
                    Style::default()
                };
                ListItem::new(d.to_string()).style(style)
            })
            .collect()
    };
    render_list_pane(
        f,
        chunks[0],
        "Devices (Tab cycles, j/k or arrows move, c connect, r refresh)",
        device_items,
        app.connection_pane == ConnectionPane::Device,
        0, // Already skipped in item generation
    );

    let mut baud_items: Vec<ListItem> = BAUD_RATES
        .iter()
        .enumerate()
        .skip(app.baud_scroll)
        .map(|(i, &b)| {
            let style = if i == app.selected_baud {
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(ratatui::style::Modifier::BOLD)
            } else {
                Style::default()
            };
            ListItem::new(b.to_string()).style(style)
        })
        .collect();
    
    // Always add custom baud rate option if it's in the visible range
    if app.baud_scroll <= BAUD_RATES.len() {
        let custom_text = if !app.custom_baud.is_empty() {
            format!("Custom: {}", app.custom_baud)
        } else if let Some(custom) = app.custom_baud_value {
            format!("Custom: {}", custom)
        } else {
            "Custom...".to_string()
        };
        let custom_style = if app.selected_baud == BAUD_RATES.len() {
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(ratatui::style::Modifier::BOLD)
        } else {
            Style::default()
        };
        baud_items.push(ListItem::new(custom_text).style(custom_style));
    }
    
    render_list_pane(
        f,
        chunks[1],
        "Baud Rate (j/k or arrows to select)",
        baud_items,
        app.connection_pane == ConnectionPane::BaudRate,
        0, // Already skipped in item generation
    );

    let data_bits_items: Vec<ListItem> = DATA_BITS
        .iter()
        .enumerate()
        .skip(app.data_bits_scroll)
        .map(|(i, bits)| {
            let style = if i == app.selected_data_bits {
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(ratatui::style::Modifier::BOLD)
            } else {
                Style::default()
            };
            ListItem::new(format!("{} bits", bits)).style(style)
        })
        .collect();
    render_list_pane(
        f,
        chunks[2],
        "Data Bits (8N1 default)",
        data_bits_items,
        app.connection_pane == ConnectionPane::DataBits,
        0, // Already skipped in item generation
    );

    let stop_bits_items: Vec<ListItem> = STOP_BITS_OPTIONS
        .iter()
        .enumerate()
        .map(|(i, bits)| {
            let style = if i == app.selected_stop_bits {
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(ratatui::style::Modifier::BOLD)
            } else {
                Style::default()
            };
            ListItem::new(format!("{} stop", bits)).style(style)
        })
        .collect();
    render_list_pane(
        f,
        chunks[3],
        "Stop Bits (1 default)",
        stop_bits_items,
        app.connection_pane == ConnectionPane::StopBits,
        0, // Stop bits list is short, no scrolling needed
    );

    let parity_items: Vec<ListItem> = PARITY_OPTIONS
        .iter()
        .enumerate()
        .map(|(i, parity)| {
            let style = if i == app.selected_parity {
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(ratatui::style::Modifier::BOLD)
            } else {
                Style::default()
            };
            ListItem::new((*parity).to_string()).style(style)
        })
        .collect();
    render_list_pane(
        f,
        chunks[4],
        "Parity (None default)",
        parity_items,
        app.connection_pane == ConnectionPane::Parity,
        0, // Parity list is short, no scrolling needed
    );
}

fn render_connected_interface(f: &mut Frame, area: Rect, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(8),
            Constraint::Length(12),
            Constraint::Length(3),
        ])
        .split(area);

    let all_lines: Vec<&str> = app.rx_buffer.lines().collect();
    let display_lines: Vec<Line> = if app.visual_mode != VisualMode::Normal && !all_lines.is_empty()
    {
        all_lines
            .iter()
            .enumerate()
            .map(|(i, line)| {
                let line = line.to_string();
                let line_len = line.len();

                if app.visual_mode == VisualMode::Selecting {
                    let start_line = app.selection_start_line.min(app.cursor_line);
                    let end_line = app.selection_start_line.max(app.cursor_line);

                    if i >= start_line && i <= end_line {
                        let sel_start = if i == start_line {
                            app.selection_start_col.min(app.cursor_col)
                        } else {
                            0
                        };
                        let sel_end = if i == end_line {
                            (app.selection_start_col.max(app.cursor_col) + 1).min(line_len)
                        } else {
                            line_len
                        };

                        let mut spans = Vec::new();
                        if sel_start > 0 {
                            spans.push(Span::raw(line[..sel_start].to_string()));
                        }
                        if sel_start < sel_end {
                            spans.push(
                                Span::raw(line[sel_start..sel_end].to_string())
                                    .bg(Color::Blue)
                                    .fg(Color::White),
                            );
                        }
                        if sel_end < line_len {
                            spans.push(Span::raw(line[sel_end..].to_string()));
                        }
                        return Line::from(spans);
                    }
                }

                if i == app.cursor_line {
                    let cursor_pos = app.cursor_col.min(line_len);
                    let mut spans = Vec::new();
                    if cursor_pos > 0 {
                        spans.push(Span::raw(line[..cursor_pos].to_string()));
                    }
                    if cursor_pos < line_len {
                        spans.push(
                            Span::raw(line[cursor_pos..cursor_pos + 1].to_string())
                                .fg(Color::Black)
                                .bg(Color::White),
                        );
                        if cursor_pos + 1 < line_len {
                            spans.push(Span::raw(line[cursor_pos + 1..].to_string()));
                        }
                    } else {
                        spans.push(Span::raw(" ").fg(Color::Black).bg(Color::White));
                    }
                    return Line::from(spans);
                }

                Line::from(line)
            })
            .collect()
    } else {
        all_lines
            .iter()
            .map(|l| Line::from(l.to_string()))
            .collect()
    };

    let rx_title = if app.visual_mode == VisualMode::Selecting {
        "Serial Data [SELECTING: j/k/h/l move, Enter copy, Esc cancel]"
    } else if app.visual_mode == VisualMode::Visual {
        "Serial Data [VISUAL: j/k/h/l move, Space select, Enter copy line, Esc exit]"
    } else {
        "Serial Data"
    };
    let scroll_offset = if app.visual_mode == VisualMode::Normal {
        let viewport_height = chunks[0].height.saturating_sub(2) as usize;
        all_lines.len().saturating_sub(viewport_height)
    } else {
        app.rx_scroll
    };
    let rx_display = Paragraph::new(display_lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(
                    if app.connected_pane == ConnectedPane::SerialData
                        || app.visual_mode != VisualMode::Normal
                    {
                        Style::default().fg(Color::Green)
                    } else {
                        Style::default().fg(Color::White)
                    },
                )
                .title(rx_title),
        )
        .scroll((scroll_offset as u16, 0));
    f.render_widget(rx_display, chunks[0]);

    let history_len = app.cmd_history.len();
    let history_items: Vec<ListItem> = if history_len == 0 {
        vec![ListItem::new("No history")]
    } else {
        app.cmd_history
            .iter()
            .rev()
            .skip(app.history_scroll)
            .take(10)
            .enumerate()
            .map(|(i, cmd)| {
                let item_index = app.history_scroll + i;
                let style = if app.connected_pane == ConnectedPane::History
                    && item_index == app.history_selected
                {
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(ratatui::style::Modifier::BOLD)
                } else {
                    Style::default()
                };
                let display_num = history_len - app.history_scroll - i;
                ListItem::new(format!("{}: {}", display_num, cmd)).style(style)
            })
            .collect()
    };
    render_list_pane(
        f,
        chunks[1],
        "History (l load, Enter send)",
        history_items,
        app.connected_pane == ConnectedPane::History,
        0, // History already manages its own scroll via skip() in item generation
    );

    let input = Paragraph::new(format!("> {}", app.tx_input))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(if app.connected_pane == ConnectedPane::Input {
                    Style::default().fg(Color::Green)
                } else {
                    Style::default().fg(Color::White)
                })
                .title("Input"),
        )
        .style(Style::default().fg(Color::Cyan));
    f.render_widget(input, chunks[2]);
}

fn main() -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::new();
    app.refresh_devices()?;

    loop {
        app.poll_rx();

        terminal.draw(|f| {
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(3),
                    Constraint::Length(3),
                    Constraint::Min(0),
                ])
                .split(f.area());

            render_header(f, chunks[0], &app);
            render_status(f, chunks[1], &app);
            match app.app_state {
                AppState::ConnectionDialog => render_connection_dialog(f, chunks[2], &app),
                AppState::Connected => render_connected_interface(f, chunks[2], &app),
            }

            if app.show_about {
                render_about_dialog(f, f.area());
            }
        })?;

        if event::poll(Duration::from_millis(16))? {
            if let Event::Key(key) = event::read()? {
                if key.kind != KeyEventKind::Press {
                    continue;
                }

                if app.show_about {
                    app.show_about = false;
                    continue;
                }

                if app.visual_mode != VisualMode::Normal {
                    match key.code {
                        KeyCode::Esc | KeyCode::Char('v') => app.exit_visual_mode(),
                        KeyCode::Char('h') => app.move_cursor("h", 10),
                        KeyCode::Char('j') => app.move_cursor("j", 10),
                        KeyCode::Char('k') => app.move_cursor("k", 10),
                        KeyCode::Char('l') => app.move_cursor("l", 10),
                        KeyCode::Char(' ') => app.start_selection(),
                        KeyCode::Enter => {
                            if app.visual_mode == VisualMode::Selecting {
                                app.copy_selection_to_clipboard();
                            } else {
                                let lines = app.get_rx_lines();
                                if let Some(line) = lines.get(app.cursor_line) {
                                    let text = line.clone();
                                    let mut copied = false;
                                    if let Some(ref mut cb) = app.clipboard {
                                        if cb.set_text(text.clone()).is_ok() {
                                            app.status_msg =
                                                format!("Copied line ({} chars)!", text.len());
                                            copied = true;
                                        }
                                    }
                                    if !copied {
                                        match arboard::Clipboard::new() {
                                            Ok(mut cb) => {
                                                if cb.set_text(text.clone()).is_ok() {
                                                    app.clipboard = Some(cb);
                                                    app.status_msg = format!(
                                                        "Copied line ({} chars)!",
                                                        text.len()
                                                    );
                                                } else {
                                                    app.status_msg = "Copy failed!".to_string();
                                                }
                                            }
                                            Err(e) => {
                                                app.status_msg = format!("Clipboard error: {}", e)
                                            }
                                        }
                                    }
                                }
                            }
                            app.exit_visual_mode();
                        }
                        _ => {}
                    }
                    continue;
                }

                let custom_baud_active = app.app_state == AppState::ConnectionDialog
                    && app.connection_pane == ConnectionPane::BaudRate
                    && app.selected_baud == BAUD_RATES.len();

                if app.app_state == AppState::Connected
                    && app.connected_pane == ConnectedPane::Input
                {
                    match key.code {
                        KeyCode::Tab => app.next_pane(),
                        KeyCode::BackTab => app.prev_pane(),
                        KeyCode::Esc => app.connected_pane = ConnectedPane::SerialData,
                        KeyCode::Enter => app.send_command(),
                        KeyCode::Backspace => {
                            app.tx_input.pop();
                        }
                        KeyCode::Char(c) => app.tx_input.push(c),
                        _ => {}
                    }
                    continue;
                }

                match key.code {
                    KeyCode::Esc => {
                        // Escape key handling
                        if custom_baud_active {
                            // Exit custom baud mode - move back to last standard baud
                            app.selected_baud = BAUD_RATES.len() - 1;
                            app.custom_baud.clear();
                            // Adjust scroll to show the selected item
                            let visible_height = 4;
                            while app.selected_baud >= app.baud_scroll + visible_height {
                                app.baud_scroll += 1;
                            }
                            while app.selected_baud < app.baud_scroll {
                                app.baud_scroll -= 1;
                            }
                            app.status_msg = "Exited custom baud mode".to_string();
                        }
                    }
                    KeyCode::Tab => app.next_pane(),
                    KeyCode::BackTab => app.prev_pane(),
                    KeyCode::Char('q') if app.app_state == AppState::ConnectionDialog && !custom_baud_active => break,
                    KeyCode::Char('q')
                        if app.app_state == AppState::Connected
                            && app.connected_pane != ConnectedPane::Input =>
                    {
                        break;
                    }
                    KeyCode::Char('?') => app.show_about = true,
                    KeyCode::Char('v')
                        if app.app_state == AppState::Connected
                            && app.connected_pane == ConnectedPane::SerialData =>
                    {
                        app.enter_visual_mode();
                    }
                    KeyCode::Char('r') if app.app_state == AppState::ConnectionDialog => {
                        app.refresh_devices().ok();
                    }
                    KeyCode::Char('c')
                        if app.app_state == AppState::ConnectionDialog && !custom_baud_active =>
                    {
                        if app.selected_baud == BAUD_RATES.len() {
                            if let Ok(custom) = app.custom_baud.parse::<u32>() {
                                app.custom_baud_value = Some(custom);
                            } else if !app.custom_baud.is_empty() {
                                app.status_msg = "Invalid baud rate".to_string();
                                continue;
                            }
                        }
                        app.connect();
                    }
                    KeyCode::Char('d') if app.app_state == AppState::Connected => app.disconnect(),
                    KeyCode::Char('l')
                        if app.app_state == AppState::Connected
                            && app.connected_pane == ConnectedPane::History =>
                    {
                        app.load_history_to_input();
                        app.connected_pane = ConnectedPane::Input;
                    }
                    KeyCode::Char('j') | KeyCode::Down => {
                        if custom_baud_active && matches!(key.code, KeyCode::Char('j')) {
                            app.custom_baud.push('j');
                            continue;
                        }
                        match app.app_state {
                            AppState::ConnectionDialog => match app.connection_pane {
                                ConnectionPane::Device => {
                                    if let Some(idx) = app.selected_device {
                                        if idx + 1 < app.devices.len() {
                                            app.selected_device = Some(idx + 1);
                                            // Auto-scroll: if selection goes beyond visible area
                                            let visible_height = 5; // Approximate visible items
                                            if idx + 1 >= app.device_scroll + visible_height {
                                                app.device_scroll = (idx + 1).saturating_sub(visible_height - 1);
                                            }
                                        }
                                    } else if !app.devices.is_empty() {
                                        app.selected_device = Some(0);
                                        app.device_scroll = 0;
                                    }
                                }
                                ConnectionPane::BaudRate => {
                                    if app.selected_baud < BAUD_RATES.len() {
                                        app.selected_baud += 1;
                                        // Auto-scroll for baud rate list
                                        // Baud pane height is 6, minus 2 for borders = 4 visible items
                                        let visible_height = 4;
                                        
                                        // Keep selected item visible: scroll down if needed
                                        while app.selected_baud >= app.baud_scroll + visible_height {
                                            app.baud_scroll += 1;
                                        }
                                    }
                                }
                                ConnectionPane::DataBits => {
                                    if app.selected_data_bits + 1 < DATA_BITS.len() {
                                        app.selected_data_bits += 1;
                                        let visible_height = 3;
                                        while app.selected_data_bits
                                            >= app.data_bits_scroll + visible_height
                                        {
                                            app.data_bits_scroll += 1;
                                        }
                                    }
                                }
                                ConnectionPane::StopBits => {
                                    if app.selected_stop_bits + 1 < STOP_BITS_OPTIONS.len() {
                                        app.selected_stop_bits += 1;
                                    }
                                }
                                ConnectionPane::Parity => {
                                    if app.selected_parity + 1 < PARITY_OPTIONS.len() {
                                        app.selected_parity += 1;
                                    }
                                }
                            },
                            AppState::Connected => match app.connected_pane {
                                ConnectedPane::SerialData => {}
                                ConnectedPane::History => {
                                    let history_len = app.cmd_history.len();
                                    if history_len > 0 {
                                        let max_selected = history_len.saturating_sub(1);
                                        if app.history_selected < max_selected {
                                            app.history_selected += 1;
                                            if app.history_selected >= app.history_scroll + 10 {
                                                app.history_scroll = app.history_selected - 9;
                                            }
                                        }
                                    }
                                }
                                ConnectedPane::Input => {}
                            },
                        }
                    }
                    KeyCode::Char('k') | KeyCode::Up => {
                        // In custom baud mode, k/Up navigates away instead of typing 'k'
                        if custom_baud_active {
                            if matches!(key.code, KeyCode::Char('k')) {
                                // For 'k' key in custom mode, navigate away (don't type 'k')
                                app.selected_baud = BAUD_RATES.len() - 1;
                                app.custom_baud.clear();
                                // Adjust scroll
                                while app.selected_baud < app.baud_scroll {
                                    app.baud_scroll -= 1;
                                }
                                app.status_msg = "".to_string();
                                continue;
                            }
                        }
                        
                        match app.app_state {
                            AppState::ConnectionDialog => match app.connection_pane {
                                ConnectionPane::Device => {
                                    if let Some(idx) = app.selected_device {
                                        if idx > 0 {
                                            app.selected_device = Some(idx - 1);
                                            // Auto-scroll: if selection goes above visible area
                                            if idx - 1 < app.device_scroll {
                                                app.device_scroll = idx - 1;
                                            }
                                        }
                                    }
                                }
                                ConnectionPane::BaudRate => {
                                    if app.selected_baud > 0 {
                                        app.selected_baud -= 1;
                                        // Auto-scroll for baud rate list
                                        if app.selected_baud < app.baud_scroll {
                                            app.baud_scroll = app.selected_baud;
                                        }
                                    }
                                }
                                ConnectionPane::DataBits => {
                                    if app.selected_data_bits > 0 {
                                        app.selected_data_bits -= 1;
                                        if app.selected_data_bits < app.data_bits_scroll {
                                            app.data_bits_scroll = app.selected_data_bits;
                                        }
                                    }
                                }
                                ConnectionPane::StopBits => {
                                    if app.selected_stop_bits > 0 {
                                        app.selected_stop_bits -= 1;
                                    }
                                }
                                ConnectionPane::Parity => {
                                    if app.selected_parity > 0 {
                                        app.selected_parity -= 1;
                                    }
                                }
                            },
                            AppState::Connected => {
                                if app.connected_pane == ConnectedPane::History
                                    && app.history_selected > 0
                                {
                                    app.history_selected -= 1;
                                    if app.history_selected < app.history_scroll {
                                        app.history_scroll = app.history_selected;
                                    }
                                }
                            }
                        }
                    }
                    KeyCode::Enter => match app.app_state {
                        AppState::ConnectionDialog => {
                            if app.connection_pane == ConnectionPane::BaudRate
                                && app.selected_baud == BAUD_RATES.len()
                            {
                                if let Ok(custom) = app.custom_baud.parse::<u32>() {
                                    app.custom_baud_value = Some(custom);
                                    app.status_msg = format!("Custom baud set to {}", custom);
                                } else if !app.custom_baud.is_empty() {
                                    app.status_msg = "Invalid baud rate".to_string();
                                }
                            }
                        }
                        AppState::Connected => match app.connected_pane {
                            ConnectedPane::History => app.send_from_history(),
                            ConnectedPane::Input => app.send_command(),
                            ConnectedPane::SerialData => {}
                        },
                    },
                    KeyCode::Backspace => {
                        if app.app_state == AppState::Connected
                            && app.connected_pane == ConnectedPane::Input
                        {
                            app.tx_input.pop();
                        } else if custom_baud_active {
                            app.custom_baud.pop();
                        }
                    }
                    KeyCode::Char(c) => {
                        if app.app_state == AppState::Connected
                            && app.connected_pane == ConnectedPane::Input
                        {
                            app.tx_input.push(c);
                        } else if custom_baud_active && c.is_ascii_digit() {
                            app.custom_baud.push(c);
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    Ok(())
}

fn render_header(f: &mut Frame, area: Rect, app: &App) {
    let connected = app.app_state == AppState::Connected;
    let status = if connected {
        "CONNECTED"
    } else {
        "DISCONNECTED"
    };
    let status_color = if connected { Color::Green } else { Color::Red };

    let pane_indicator = match app.app_state {
        AppState::ConnectionDialog => format!(
            "[{}]",
            connection_pane_title(app.connection_pane).to_uppercase()
        ),
        AppState::Connected => format!(
            "[{}]",
            connected_pane_title(app.connected_pane).to_uppercase()
        ),
    };

    let detail = match app.app_state {
        AppState::ConnectionDialog => {
            let baud = if app.selected_baud < BAUD_RATES.len() {
                BAUD_RATES[app.selected_baud].to_string()
            } else {
                app.custom_baud_value
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "Custom".to_string())
            };
            format!("Selected baud: {}", baud)
        }
        AppState::Connected => format!("{} @ {}", app.connected_device, app.connected_baud),
    };

    let text = vec![Line::from(vec![
        Span::raw("Serial TUI "),
        Span::raw(pane_indicator).fg(Color::Cyan),
        Span::raw(" | "),
        Span::raw(status).fg(status_color),
        Span::raw(" | "),
        Span::raw(detail),
        Span::raw(" | Tab/Shift+Tab panes ?:about"),
    ])];

    let paragraph = Paragraph::new(text)
        .style(Style::default().fg(Color::White))
        .block(Block::default().borders(Borders::ALL).title("Status"));
    f.render_widget(paragraph, area);
}

fn render_status(f: &mut Frame, area: Rect, app: &App) {
    let help_text = match app.app_state {
        AppState::ConnectionDialog => match app.connection_pane {
            ConnectionPane::Device => "j/k or arrows: select device  c:connect  r:refresh  q:quit",
            ConnectionPane::BaudRate => {
                if app.selected_baud == BAUD_RATES.len() {
                    "Type digits for custom baud  Backspace:delete  Esc or k:exit custom  c:connect"
                } else {
                    "j/k or arrows: select baud  j to end: custom baud  c:connect"
                }
            }
            ConnectionPane::DataBits => "j/k or arrows: select data bits",
            ConnectionPane::StopBits => "j/k or arrows: select stop bits",
            ConnectionPane::Parity => "j/k or arrows: select parity",
        },
        AppState::Connected => match app.connected_pane {
            ConnectedPane::SerialData => "v:visual mode  d:disconnect  q:quit",
            ConnectedPane::History => "j/k:scroll  l:load to input  Enter:send  d:disconnect  q:quit",
            ConnectedPane::Input => "type command  Enter:send  Tab/Shift+Tab:panes  Esc:leave input",
        },
    };
    let full_text = if app.status_msg.is_empty() {
        help_text.to_string()
    } else {
        format!("{} | {}", app.status_msg, help_text)
    };
    let paragraph = Paragraph::new(vec![Line::from(full_text)])
        .style(Style::default().fg(Color::Yellow))
        .block(Block::default().borders(Borders::ALL).title("Help"));
    f.render_widget(paragraph, area);
}

fn render_about_dialog(f: &mut Frame, area: Rect) {
    // Create a centered popup
    let popup_width = 60;
    let popup_height = 10;
    let popup_area = Rect {
        x: (area.width.saturating_sub(popup_width)) / 2,
        y: (area.height.saturating_sub(popup_height)) / 2,
        width: popup_width.min(area.width),
        height: popup_height.min(area.height),
    };

    // Clear background
    let clear_block = Block::default().style(Style::default().bg(Color::Black));
    f.render_widget(clear_block, popup_area);

    let text = vec![
        Line::from(""),
        Line::from(vec![
            Span::raw("            Vibe coded with "),
            Span::raw("♥").fg(Color::Red),
        ]),
        Line::from("         using OpenCode and Copilot"),
        Line::from(""),
        Line::from("          MontyTheSoftwareEngineer").fg(Color::Cyan),
        Line::from(""),
        Line::from("  https://github.com/MontyTheSoftwareEngineer/serial-tui").fg(Color::Blue),
        Line::from(""),
        Line::from("         Press any key to continue...").fg(Color::Yellow),
    ];

    let paragraph = Paragraph::new(text)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Magenta))
                .title(" About "),
        )
        .style(Style::default().fg(Color::White).bg(Color::Black));

    f.render_widget(paragraph, popup_area);
}

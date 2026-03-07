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
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::Duration;

const BAUD_RATES: &[u32] = &[9600, 19200, 38400, 57600, 115200, 230400, 460800, 921600];

fn get_history_path() -> std::path::PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    std::path::PathBuf::from(home).join(".serial-tui_history")
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
enum ActivePane {
    Devices,
    Baud,
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
    devices: Vec<String>,
    device_paths: Vec<String>,
    selected_device: Option<usize>,
    baud_rate: usize,
    connected: bool,
    rx_buffer: String,
    tx_input: String,
    tx_sender: Option<Sender<String>>,
    rx_receiver: Option<Receiver<String>>,
    cmd_history: Vec<String>,
    history_index: Option<usize>,
    status_msg: String,
    active_pane: ActivePane,
    baud_selected: Option<usize>,
    history_scroll: usize,
    visual_mode: VisualMode,
    cursor_line: usize,
    cursor_col: usize,
    selection_start_line: usize,
    selection_start_col: usize,
}

impl App {
    fn new() -> Self {
        let cmd_history = load_history();
        Self {
            devices: Vec::new(),
            device_paths: Vec::new(),
            selected_device: None,
            baud_rate: 4,
            connected: false,
            rx_buffer: String::new(),
            tx_input: String::new(),
            tx_sender: None,
            rx_receiver: None,
            cmd_history,
            history_index: None,
            status_msg: String::new(),
            active_pane: ActivePane::Devices,
            baud_selected: None,
            history_scroll: 0,
            visual_mode: VisualMode::Normal,
            cursor_line: 0,
            cursor_col: 0,
            selection_start_line: 0,
            selection_start_col: 0,
        }
    }

    fn refresh_devices(&mut self) -> Result<()> {
        let ports = serialport::available_ports()?;
        self.devices.clear();
        self.device_paths.clear();
        for port in ports {
            let path = port.port_name.clone();
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
        let baud = BAUD_RATES[self.baud_rate];

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
        thread::spawn(move || {
            let mut port = port_read;
            let mut buf = [0u8; 2048];
            loop {
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

        thread::spawn(move || {
            let mut port = port_write;
            while let Ok(cmd) = rx_cmd.recv() {
                let _ = port.write_all(cmd.as_bytes());
                let _ = port.write_all(b"\r\n");
                let _ = port.flush();
            }
        });

        self.connected = true;
        self.status_msg = format!("Connected to {} at {}", device_path, baud);
    }

    fn disconnect(&mut self) {
        self.tx_sender = None;
        self.rx_receiver = None;
        self.connected = false;
        self.status_msg = "Disconnected".to_string();
    }

    fn send_command(&mut self) {
        if !self.tx_input.is_empty() {
            if let Some(tx) = &self.tx_sender {
                let cmd = self.tx_input.clone();
                let _ = tx.send(cmd);
                self.rx_buffer.push_str(&format!("> {}\n", self.tx_input));
            }
            self.cmd_history.push(self.tx_input.clone());
            self.history_index = None;
            self.history_scroll = 0;
            self.tx_input.clear();
            save_history(&self.cmd_history);
        }
    }

    fn send_from_history(&mut self) {
        if let Some(idx) = self.history_index {
            if let Some(cmd) = self.cmd_history.get(idx) {
                if let Some(tx) = &self.tx_sender {
                    let cmd = cmd.clone();
                    let _ = tx.send(cmd.clone());
                    self.rx_buffer.push_str(&format!("> {}\n", cmd));
                }
                self.history_scroll = 0;
            }
        }
    }

    fn load_history_to_input(&mut self) {
        // history_scroll points to the top visible item, use that
        if !self.cmd_history.is_empty() {
            let idx = self.cmd_history.len() - 1 - self.history_scroll;
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
        self.cursor_line = 0;
        self.cursor_col = 0;
        self.selection_start_line = 0;
        self.selection_start_col = 0;
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
                result.push_str(&line[start_col..end_col.min(line.len())]);
            } else if i == start_line {
                result.push_str(&line[start_col..]);
                result.push('\n');
            } else if i == end_line {
                result.push_str(&line[..end_col.min(line.len())]);
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
        match arboard::Clipboard::new() {
            Ok(mut clipboard) => {
                if let Err(e) = clipboard.set_text(&text) {
                    self.status_msg = format!("Clipboard error: {}", e);
                    return false;
                }
                self.status_msg = "Copied to clipboard".to_string();
                true
            }
            Err(e) => {
                self.status_msg = format!("Clipboard error: {}", e);
                false
            }
        }
    }

    fn move_cursor(&mut self, direction: &str) {
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
                }
            }
            "k" => {
                if self.cursor_line > 0 {
                    self.cursor_line -= 1;
                    self.cursor_col = self
                        .cursor_col
                        .min(lines.get(self.cursor_line).map(|l| l.len()).unwrap_or(0));
                }
            }
            "l" => {
                if self.cursor_col < max_col {
                    self.cursor_col += 1;
                } else if self.cursor_line < max_line {
                    self.cursor_line += 1;
                    self.cursor_col = 0;
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

    fn next_pane(&mut self) {
        self.active_pane = match self.active_pane {
            ActivePane::Devices => ActivePane::Baud,
            ActivePane::Baud => ActivePane::History,
            ActivePane::History => ActivePane::Input,
            ActivePane::Input => ActivePane::Devices,
        };
        if self.active_pane == ActivePane::Baud {
            self.baud_selected = Some(self.baud_rate);
        }
        if self.active_pane == ActivePane::History && self.cmd_history.is_empty() == false {
            self.history_scroll = 0;
        }
    }

    fn prev_pane(&mut self) {
        self.active_pane = match self.active_pane {
            ActivePane::Devices => ActivePane::Input,
            ActivePane::Baud => ActivePane::Devices,
            ActivePane::History => ActivePane::Baud,
            ActivePane::Input => ActivePane::History,
        };
        if self.active_pane == ActivePane::Baud {
            self.baud_selected = Some(self.baud_rate);
        }
    }
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
                    Constraint::Length(3),
                ])
                .split(f.area());

            render_header(f, chunks[0], &app);
            render_status(f, chunks[1], &app);
            render_main(f, chunks[2], &app);
            render_input(f, chunks[3], &app);
        })?;

        if event::poll(Duration::from_millis(16))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    // Handle visual mode keys
                    if app.visual_mode != VisualMode::Normal {
                        match key.code {
                            KeyCode::Esc => {
                                app.exit_visual_mode();
                            }
                            KeyCode::Char('v') => {
                                app.exit_visual_mode();
                            }
                            KeyCode::Char('h') => {
                                app.move_cursor("h");
                            }
                            KeyCode::Char('j') => {
                                app.move_cursor("j");
                            }
                            KeyCode::Char('k') => {
                                app.move_cursor("k");
                            }
                            KeyCode::Char('l') => {
                                app.move_cursor("l");
                            }
                            KeyCode::Char(' ') => {
                                app.start_selection();
                            }
                            KeyCode::Enter => {
                                if app.visual_mode == VisualMode::Selecting {
                                    app.copy_selection_to_clipboard();
                                    app.exit_visual_mode();
                                }
                            }
                            _ => {}
                        }
                    } else {
                        match key.code {
                            KeyCode::Tab => {
                                app.next_pane();
                            }
                            KeyCode::BackTab => {
                                app.prev_pane();
                            }
                            KeyCode::Char('q') => break,
                            KeyCode::Char('v') => {
                                app.enter_visual_mode();
                            }
                            KeyCode::Char('r') => {
                                app.refresh_devices().ok();
                            }
                            KeyCode::Char('c') if app.active_pane == ActivePane::Devices => {
                                if app.connected {
                                    app.disconnect();
                                } else {
                                    app.connect();
                                }
                            }
                            KeyCode::Char('b') if app.active_pane == ActivePane::Devices => {
                                app.active_pane = ActivePane::Baud;
                                app.baud_selected = Some(app.baud_rate);
                            }
                            KeyCode::Char('j') | KeyCode::Down => match app.active_pane {
                                ActivePane::Devices => {
                                    if let Some(idx) = app.selected_device {
                                        if idx < app.devices.len() - 1 {
                                            app.selected_device = Some(idx + 1);
                                        }
                                    } else if !app.devices.is_empty() {
                                        app.selected_device = Some(0);
                                    }
                                }
                                ActivePane::Baud => {
                                    if let Some(idx) = app.baud_selected {
                                        if idx < BAUD_RATES.len() - 1 {
                                            app.baud_selected = Some(idx + 1);
                                        }
                                    } else {
                                        app.baud_selected = Some(0);
                                    }
                                }
                                ActivePane::History => {
                                    let max_scroll = app.cmd_history.len().saturating_sub(1);
                                    if app.history_scroll < max_scroll {
                                        app.history_scroll += 1;
                                    }
                                }
                                ActivePane::Input => {
                                    // Move cursor to end of input
                                    // This allows typing at end
                                }
                            },
                            KeyCode::Char('k') | KeyCode::Up => match app.active_pane {
                                ActivePane::Devices => {
                                    if let Some(idx) = app.selected_device {
                                        if idx > 0 {
                                            app.selected_device = Some(idx - 1);
                                        }
                                    }
                                }
                                ActivePane::Baud => {
                                    if let Some(idx) = app.baud_selected {
                                        if idx > 0 {
                                            app.baud_selected = Some(idx - 1);
                                        }
                                    }
                                }
                                ActivePane::History => {
                                    if app.history_scroll > 0 {
                                        app.history_scroll -= 1;
                                    }
                                }
                                ActivePane::Input => {}
                            },
                            KeyCode::Char('l') if app.active_pane == ActivePane::History => {
                                app.load_history_to_input();
                                app.active_pane = ActivePane::Input;
                            }
                            KeyCode::Enter => match app.active_pane {
                                ActivePane::Baud => {
                                    if let Some(idx) = app.baud_selected {
                                        app.baud_rate = idx;
                                    }
                                    app.active_pane = ActivePane::Devices;
                                }
                                ActivePane::Input => {
                                    app.send_command();
                                }
                                ActivePane::History => {
                                    app.send_from_history();
                                }
                                ActivePane::Devices => {}
                            },
                            KeyCode::Backspace => {
                                if app.active_pane == ActivePane::Input {
                                    app.tx_input.pop();
                                }
                            }
                            KeyCode::Left => {
                                // Just ignore, can't easily do cursor movement in simple input
                            }
                            KeyCode::Right => {
                                // Just ignore
                            }
                            KeyCode::Char(c) => {
                                if app.active_pane == ActivePane::Input {
                                    app.tx_input.push(c);
                                }
                            }
                            _ => {}
                        }
                    }
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
    let status = if app.connected {
        "CONNECTED"
    } else {
        "DISCONNECTED"
    };
    let status_color = if app.connected {
        Color::Green
    } else {
        Color::Red
    };
    let baud = BAUD_RATES[app.baud_rate];

    let pane_indicator = match app.active_pane {
        ActivePane::Devices => "[DEVICES]",
        ActivePane::Baud => "[BAUD]",
        ActivePane::History => "[HISTORY]",
        ActivePane::Input => "[INPUT]",
    };

    let text = vec![Line::from(vec![
        Span::raw("Serial TUI "),
        Span::raw(pane_indicator).fg(Color::Cyan),
        Span::raw(" | "),
        Span::raw(status).fg(status_color),
        Span::raw(" | Baud: "),
        Span::raw(baud.to_string()),
        Span::raw(" | Tab:next Shift-Tab:prev "),
        Span::raw("q:quit"),
    ])];

    let paragraph = Paragraph::new(text)
        .style(Style::default().fg(Color::White))
        .block(Block::default().borders(Borders::ALL).title("Status"));
    f.render_widget(paragraph, area);
}

fn render_status(f: &mut Frame, area: Rect, app: &App) {
    let help_text = match app.active_pane {
        ActivePane::Devices => "j/k:select c:connect b:baud r:refresh",
        ActivePane::Baud => "j/k:select baud Enter:confirm",
        ActivePane::History => "j/k:scroll l:load to input Enter:send",
        ActivePane::Input => "type command Enter:send",
    };
    let full_text = if app.status_msg.is_empty() {
        help_text.to_string()
    } else {
        format!("{} | {}", app.status_msg, help_text)
    };
    let text = vec![Line::from(full_text.as_str())];
    let paragraph = Paragraph::new(text)
        .style(Style::default().fg(Color::Yellow))
        .block(Block::default().borders(Borders::ALL).title("Help"));
    f.render_widget(paragraph, area);
}

fn render_main(f: &mut Frame, area: Rect, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(20),
            Constraint::Percentage(20),
            Constraint::Percentage(60),
        ])
        .split(area);

    // Devices pane
    let devices_active = app.active_pane == ActivePane::Devices;
    let items: Vec<ListItem> = app
        .devices
        .iter()
        .enumerate()
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
        .collect();

    let list = List::new(items)
        .block(
            Block::default()
                .borders(if devices_active {
                    Borders::ALL
                } else {
                    Borders::NONE
                })
                .border_style(if devices_active {
                    Style::default().fg(Color::Cyan)
                } else {
                    Style::default()
                })
                .title("Devices (j/k)"),
        )
        .highlight_style(Style::default().fg(Color::Yellow));

    f.render_widget(list, chunks[0]);

    // Baud pane
    let baud_active = app.active_pane == ActivePane::Baud;
    let baud_items: Vec<ListItem> = BAUD_RATES
        .iter()
        .enumerate()
        .map(|(i, &b)| {
            let style = if Some(i) == app.baud_selected {
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(ratatui::style::Modifier::BOLD)
            } else {
                Style::default()
            };
            ListItem::new(b.to_string()).style(style)
        })
        .collect();

    let baud_list = List::new(baud_items)
        .block(
            Block::default()
                .borders(if baud_active {
                    Borders::ALL
                } else {
                    Borders::NONE
                })
                .border_style(if baud_active {
                    Style::default().fg(Color::Cyan)
                } else {
                    Style::default()
                })
                .title("Baud (j/k)"),
        )
        .highlight_style(Style::default().fg(Color::Yellow));

    f.render_widget(baud_list, chunks[1]);

    // RX/TX area and History pane (vertical split within right column)
    let rx_chunk = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(0), Constraint::Length(12)])
        .split(chunks[2]);

    // RX/TX - show at top
    let all_lines_vec: Vec<String> = app.rx_buffer.lines().map(|s| s.to_string()).collect();

    // Build lines with cursor/selection highlighting
    let display_lines: Vec<Line> =
        if app.visual_mode != VisualMode::Normal && !all_lines_vec.is_empty() {
            all_lines_vec
                .iter()
                .enumerate()
                .map(|(line_idx, line)| {
                    let line_len = line.len();
                    let mut spans = Vec::new();

                    if line_idx == app.cursor_line {
                        let cursor_pos = app.cursor_col.min(line_len);

                        if app.visual_mode == VisualMode::Selecting {
                            let start_line = app.selection_start_line.min(app.cursor_line);
                            let end_line = app.selection_start_line.max(app.cursor_line);

                            if line_idx >= start_line && line_idx <= end_line {
                                let sel_start = if line_idx == start_line {
                                    app.selection_start_line.min(app.cursor_col)
                                } else {
                                    0
                                };
                                let sel_end = if line_idx == end_line {
                                    app.selection_start_col.max(app.cursor_col)
                                } else {
                                    line_len
                                };
                                let sel_end = sel_end.min(line_len);

                                // Before selection
                                if cursor_pos > sel_start {
                                    spans.push(Span::raw(&line[..sel_start]));
                                }
                                // Selected text
                                if sel_start < sel_end {
                                    spans.push(
                                        Span::raw(&line[sel_start..sel_end])
                                            .bg(Color::Blue)
                                            .fg(Color::White),
                                    );
                                }
                                // After selection
                                if cursor_pos > sel_end {
                                    spans.push(Span::raw(&line[sel_end..]));
                                }
                            } else {
                                spans.push(Span::raw(line.as_str()));
                            }
                        } else {
                            // Visual mode (no selection yet)
                            if cursor_pos < line_len {
                                spans.push(Span::raw(&line[..cursor_pos]));
                                spans.push(
                                    Span::raw(&line[cursor_pos..cursor_pos + 1])
                                        .fg(Color::Cyan)
                                        .add_modifier(ratatui::style::Modifier::REVERSED),
                                );
                                if cursor_pos + 1 < line_len {
                                    spans.push(Span::raw(&line[cursor_pos + 1..]));
                                }
                            } else {
                                spans.push(Span::raw(line.as_str()));
                                spans.push(
                                    Span::raw(" ")
                                        .fg(Color::Cyan)
                                        .add_modifier(ratatui::style::Modifier::REVERSED),
                                );
                            }
                        }
                    } else {
                        spans.push(Span::raw(line.as_str()));
                    }

                    Line::from(spans)
                })
                .collect()
        } else {
            all_lines_vec
                .iter()
                .rev()
                .take(100)
                .rev()
                .map(|l| Line::from(l.to_string()))
                .collect()
        };

    let rx_title = if app.visual_mode != VisualMode::Normal {
        "RX/TX [VISUAL: h/j/k/l move, Space=select, Enter=copy, Esc=exit]"
    } else {
        "RX/TX"
    };

    let visual_active = app.visual_mode != VisualMode::Normal;
    let rx_display = Paragraph::new(display_lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(if visual_active {
                    Style::default().fg(Color::Green)
                } else {
                    Style::default().fg(Color::White)
                })
                .title(rx_title),
        )
        .scroll((0, 0));
    f.render_widget(rx_display, rx_chunk[0]);

    // History pane - separate selectable pane
    let history_active = app.active_pane == ActivePane::History;
    let history_len = app.cmd_history.len();
    let history_items: Vec<ListItem> = app
        .cmd_history
        .iter()
        .rev()
        .skip(app.history_scroll)
        .take(10)
        .enumerate()
        .map(|(i, cmd)| {
            let style = if history_active && i == 0 {
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(ratatui::style::Modifier::BOLD)
            } else {
                Style::default()
            };
            let display_num = history_len - app.history_scroll - i;
            ListItem::new(format!("{}: {}", display_num, cmd)).style(style)
        })
        .collect();

    let history_list = List::new(history_items).block(
        Block::default()
            .borders(if history_active {
                Borders::ALL
            } else {
                Borders::NONE
            })
            .border_style(if history_active {
                Style::default().fg(Color::Cyan)
            } else {
                Style::default()
            })
            .title("History (j/k/l/Enter)"),
    );

    f.render_widget(history_list, rx_chunk[1]);
}

fn render_input(f: &mut Frame, area: Rect, app: &App) {
    let input_active = app.active_pane == ActivePane::Input;
    let text = format!("> {}", app.tx_input);
    let paragraph = Paragraph::new(text)
        .block(
            Block::default()
                .borders(if input_active {
                    Borders::ALL
                } else {
                    Borders::NONE
                })
                .border_style(if input_active {
                    Style::default().fg(Color::Cyan)
                } else {
                    Style::default()
                })
                .title("Input"),
        )
        .style(Style::default().fg(Color::Cyan));
    f.render_widget(paragraph, area);
}

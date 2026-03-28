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
                || path.starts_with("/dev/ttyS") // Old-style serial ports usually not connected
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

        let baud = if let Some(idx) = self.baud_selected {
            // Check if it's the custom baud option (last in list)
            if idx == BAUD_RATES.len() {
                // Use custom baud if available
                self.custom_baud_value.unwrap_or(BAUD_RATES[self.baud_rate])
            } else {
                BAUD_RATES[idx]
            }
        } else {
            BAUD_RATES[self.baud_rate]
        };

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

        self.connected = true;
        self.status_msg = format!("Connected to {} at {}", device_path, baud);
        self.active_pane = ActivePane::Input;
    }

    fn disconnect(&mut self) {
        self.should_stop.store(true, Ordering::SeqCst);
        self.tx_sender = None;
        self.rx_receiver = None;
        self.connected = false;
        self.status_msg = "Disconnected".to_string();
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
            
            // Render about dialog on top if shown
            if app.show_about {
                render_about_dialog(f, f.area());
            }
        })?;

        if event::poll(Duration::from_millis(16))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    // Handle about dialog
                    if app.show_about {
                        app.show_about = false;
                        continue;
                    }
                    
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
                                if app.cursor_col > 0 {
                                    app.cursor_col -= 1;
                                } else if app.cursor_line > 0 {
                                    app.cursor_line -= 1;
                                    let line_len = app
                                        .get_rx_lines()
                                        .get(app.cursor_line)
                                        .map(|l| l.len())
                                        .unwrap_or(0);
                                    app.cursor_col = line_len;
                                }
                            }
                            KeyCode::Char('j') => {
                                let lines = app.get_rx_lines();
                                if app.cursor_line < lines.len() - 1 {
                                    app.cursor_line += 1;
                                    app.cursor_col = app.cursor_col.min(
                                        lines.get(app.cursor_line).map(|l| l.len()).unwrap_or(0),
                                    );
                                    if app.cursor_line > app.rx_scroll + 10 {
                                        app.rx_scroll = app.cursor_line - 10;
                                    }
                                } else if app.rx_scroll < lines.len().saturating_sub(1) {
                                    app.rx_scroll += 1;
                                }
                            }
                            KeyCode::Char('k') => {
                                if app.cursor_line > 0 {
                                    app.cursor_line -= 1;
                                    let lines = app.get_rx_lines();
                                    app.cursor_col = app.cursor_col.min(
                                        lines.get(app.cursor_line).map(|l| l.len()).unwrap_or(0),
                                    );
                                    if app.cursor_line < app.rx_scroll {
                                        app.rx_scroll = app.cursor_line;
                                    }
                                } else if app.rx_scroll > 0 {
                                    app.rx_scroll -= 1;
                                }
                            }
                            KeyCode::Char('l') => {
                                let lines = app.get_rx_lines();
                                let max_col =
                                    lines.get(app.cursor_line).map(|l| l.len()).unwrap_or(0);
                                if app.cursor_col < max_col {
                                    app.cursor_col += 1;
                                } else if app.cursor_line < lines.len() - 1 {
                                    app.cursor_line += 1;
                                    app.cursor_col = 0;
                                    if app.cursor_line > app.rx_scroll + 10 {
                                        app.rx_scroll = app.cursor_line - 10;
                                    }
                                }
                            }
                            KeyCode::Char(' ') => {
                                app.start_selection();
                            }
                            KeyCode::Enter => {
                                if app.visual_mode == VisualMode::Selecting {
                                    app.copy_selection_to_clipboard();
                                    app.exit_visual_mode();
                                } else if app.visual_mode == VisualMode::Visual {
                                    // If not selecting, copy the current line
                                    let lines = app.get_rx_lines();
                                    if let Some(line) = lines.get(app.cursor_line) {
                                        let text = line.clone();
                                        
                                        // Try persistent clipboard first
                                        let mut copied = false;
                                        if let Some(ref mut cb) = app.clipboard {
                                            if cb.set_text(text.clone()).is_ok() {
                                                app.status_msg = format!("Copied line ({} chars)!", text.len());
                                                copied = true;
                                            }
                                        }
                                        
                                        // Fallback to new clipboard
                                        if !copied {
                                            match arboard::Clipboard::new() {
                                                Ok(mut cb) => {
                                                    if cb.set_text(text.clone()).is_ok() {
                                                        app.clipboard = Some(cb);
                                                        app.status_msg = format!("Copied line ({} chars)!", text.len());
                                                    } else {
                                                        app.status_msg = "Copy failed!".to_string();
                                                    }
                                                }
                                                Err(e) => {
                                                    app.status_msg = format!("Clipboard error: {}", e);
                                                }
                                            }
                                        }
                                    }
                                    app.exit_visual_mode();
                                }
                            }
                            _ => {}
                        }
                    } else {
                        // Check if we're in custom baud input mode
                        let in_custom_baud_mode = app.active_pane == ActivePane::Baud 
                            && app.baud_selected == Some(BAUD_RATES.len());
                        
                        match key.code {
                            KeyCode::Tab => {
                                app.next_pane();
                            }
                            KeyCode::BackTab => {
                                app.prev_pane();
                            }
                            KeyCode::Char('q') if app.active_pane != ActivePane::Input => break,
                            KeyCode::Char('?') if app.active_pane != ActivePane::Input => {
                                app.show_about = true;
                            }
                            KeyCode::Char('v') if app.active_pane != ActivePane::Input => {
                                app.enter_visual_mode();
                            }
                            KeyCode::Char('r') if app.active_pane != ActivePane::Input => {
                                app.refresh_devices().ok();
                            }
                            KeyCode::Char('c')
                                if !in_custom_baud_mode && (app.active_pane == ActivePane::Devices
                                    || app.active_pane == ActivePane::Baud) =>
                            {
                                if app.connected {
                                    app.disconnect();
                                } else {
                                    // If in Baud pane, use the currently highlighted baud rate
                                    if app.active_pane == ActivePane::Baud {
                                        if let Some(idx) = app.baud_selected {
                                            app.baud_rate = idx;
                                        }
                                    } else if app.baud_selected.is_none() {
                                        app.baud_selected = Some(app.baud_rate);
                                    }
                                    app.connect();
                                }
                            }
                            KeyCode::Char('d')
                                if !in_custom_baud_mode && (app.active_pane == ActivePane::Devices
                                    || app.active_pane == ActivePane::Baud) =>
                            {
                                if app.connected {
                                    app.disconnect();
                                }
                            }
                            KeyCode::Char('b') if !in_custom_baud_mode && app.active_pane == ActivePane::Devices => {
                                app.active_pane = ActivePane::Baud;
                                app.baud_selected = Some(app.baud_rate);
                            }
                            KeyCode::Char('j') if app.active_pane != ActivePane::Input => {
                                if in_custom_baud_mode {
                                    // In custom baud mode, 'j' types 'j'
                                    app.custom_baud.push('j');
                                } else {
                                    match app.active_pane {
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
                                        } else if idx == BAUD_RATES.len() - 1 {
                                            // Move to custom option
                                            app.baud_selected = Some(BAUD_RATES.len());
                                        }
                                    } else {
                                        app.baud_selected = Some(0);
                                    }
                                }
                                ActivePane::History => {
                                    let history_len = app.cmd_history.len();
                                    if history_len > 0 {
                                        let max_selected = history_len.saturating_sub(1);
                                        if app.history_selected < max_selected {
                                            app.history_selected += 1;
                                            // Scroll only if selection goes beyond visible area
                                            // Visible area shows 10 items starting from history_scroll
                                            if app.history_selected >= app.history_scroll + 10 {
                                                app.history_scroll = app.history_selected - 9;
                                            }
                                        }
                                    }
                                }
                                ActivePane::Input => {
                                    // Move cursor to end of input
                                    // This allows typing at end
                                }
                            }
                        }}
                            KeyCode::Char('k') if app.active_pane != ActivePane::Input => {
                                if in_custom_baud_mode {
                                    // In custom baud mode, 'k' types 'k'
                                    app.custom_baud.push('k');
                                } else {
                                    match app.active_pane {
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
                                    if app.history_selected > 0 {
                                        app.history_selected -= 1;
                                        // Scroll only if selection goes above visible area
                                        if app.history_selected < app.history_scroll {
                                            app.history_scroll = app.history_selected;
                                        }
                                    }
                                }
                                ActivePane::Input => {}
                            }
                        }},
                            KeyCode::Char('l') if app.active_pane == ActivePane::History => {
                                app.load_history_to_input();
                                app.active_pane = ActivePane::Input;
                            }
                            KeyCode::Down if app.active_pane != ActivePane::Input => {
                                match app.active_pane {
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
                                            } else if idx == BAUD_RATES.len() - 1 {
                                                app.baud_selected = Some(BAUD_RATES.len());
                                            }
                                        } else {
                                            app.baud_selected = Some(0);
                                        }
                                    }
                                    ActivePane::History => {
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
                                    ActivePane::Input => {}
                                }
                            }
                            KeyCode::Up if app.active_pane != ActivePane::Input => {
                                match app.active_pane {
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
                                        if app.history_selected > 0 {
                                            app.history_selected -= 1;
                                            if app.history_selected < app.history_scroll {
                                                app.history_scroll = app.history_selected;
                                            }
                                        }
                                    }
                                    ActivePane::Input => {}
                                }
                            }
                            KeyCode::Enter => match app.active_pane {
                                ActivePane::Baud => {
                                    if let Some(idx) = app.baud_selected {
                                        // If custom baud is selected, try to parse the input
                                        if idx == BAUD_RATES.len() {
                                            if let Ok(custom) = app.custom_baud.parse::<u32>() {
                                                app.custom_baud_value = Some(custom);
                                                app.baud_rate = idx;
                                                app.status_msg = format!("Custom baud set to {}", custom);
                                            } else if !app.custom_baud.is_empty() {
                                                app.status_msg = "Invalid baud rate".to_string();
                                            }
                                        } else {
                                            app.baud_rate = idx;
                                        }
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
                                } else if app.active_pane == ActivePane::Baud && app.baud_selected == Some(BAUD_RATES.len()) {
                                    // Allow editing custom baud
                                    app.custom_baud.pop();
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
                                } else if app.active_pane == ActivePane::Baud && app.baud_selected == Some(BAUD_RATES.len()) {
                                    // Allow typing custom baud (only digits)
                                    if c.is_ascii_digit() {
                                        app.custom_baud.push(c);
                                    }
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
        Span::raw(" | Tab/Shift+Tab:switch panes v:visual q:quit ?:about"),
    ])];

    let paragraph = Paragraph::new(text)
        .style(Style::default().fg(Color::White))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::White))
                .title("Status"),
        );
    f.render_widget(paragraph, area);
}

fn render_status(f: &mut Frame, area: Rect, app: &App) {
    let help_text = match app.active_pane {
        ActivePane::Devices => "j/k:select c/d:connect/disconnect b:baud r:refresh",
        ActivePane::Baud => "j/k:select type:custom c/d:connect Enter:confirm",
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
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::White))
                .title("Help"),
        );
    f.render_widget(paragraph, area);
}

fn render_main(f: &mut Frame, area: Rect, app: &App) {
    // Split into top (devices+baud) and bottom (rx+history)
    let main_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(12), // Devices and Baud at top
            Constraint::Min(0),      // RX/TX and History
        ])
        .split(area);

    // Split top section horizontally for devices and baud
    let top_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(50),
            Constraint::Percentage(50),
        ])
        .split(main_chunks[0]);

    // Split bottom section horizontally for RX and History
    let bottom_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(70),
            Constraint::Percentage(30),
        ])
        .split(main_chunks[1]);

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
                .borders(Borders::ALL)
                .border_style(if devices_active {
                    Style::default().fg(Color::Green)
                } else {
                    Style::default().fg(Color::White)
                })
                .title("Devices (j/k)"),
        )
        .highlight_style(Style::default().fg(Color::Yellow));

    f.render_widget(list, top_chunks[0]);

    // Baud pane
    let baud_active = app.active_pane == ActivePane::Baud;
    let mut baud_items: Vec<ListItem> = BAUD_RATES
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
    
    // Add custom baud option
    let custom_text = if let Some(custom) = app.custom_baud_value {
        format!("Custom: {}", custom)
    } else if !app.custom_baud.is_empty() {
        format!("Custom: {}", app.custom_baud)
    } else {
        "Custom...".to_string()
    };
    let custom_style = if Some(BAUD_RATES.len()) == app.baud_selected {
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(ratatui::style::Modifier::BOLD)
    } else {
        Style::default()
    };
    baud_items.push(ListItem::new(custom_text).style(custom_style));

    let baud_list = List::new(baud_items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(if baud_active {
                    Style::default().fg(Color::Green)
                } else {
                    Style::default().fg(Color::White)
                })
                .title("Baud (j/k)"),
        )
        .highlight_style(Style::default().fg(Color::Yellow));

    f.render_widget(baud_list, top_chunks[1]);

    // RX/TX - show on left of bottom section
    let rx_area = bottom_chunks[0];
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
                        // Include character at cursor position (+1)
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

                // Show cursor position
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
                        spans.push(Span::raw(" ".to_string()).fg(Color::Black).bg(Color::White));
                    }
                    return Line::from(spans);
                }

                Line::from(line)
            })
            .collect()
    } else {
        // Normal mode - show all lines, let Paragraph handle scrolling
        all_lines
            .iter()
            .map(|l| Line::from(l.to_string()))
            .collect()
    };

    let rx_title = if app.visual_mode == VisualMode::Selecting {
        "RX/TX [SELECTING: j/k/h/l move, Enter=copy selection, Esc=cancel]"
    } else if app.visual_mode == VisualMode::Visual {
        "RX/TX [VISUAL: j/k/h/l move, Space=start select, Enter=copy line, Esc=exit]"
    } else {
        "RX/TX"
    };

    let visual_active = app.visual_mode != VisualMode::Normal;
    
    // Calculate scroll for autoscroll in normal mode
    let scroll_offset = if app.visual_mode == VisualMode::Normal {
        // In normal mode, autoscroll to bottom
        let viewport_height = rx_area.height.saturating_sub(2) as usize; // subtract borders
        let line_count = all_lines.len();
        line_count.saturating_sub(viewport_height)
    } else {
        // In visual mode, use manual scroll
        app.rx_scroll
    };
    
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
        .scroll((scroll_offset as u16, 0));
    f.render_widget(rx_display, rx_area);

    // History pane - separate selectable pane
    let history_active = app.active_pane == ActivePane::History;
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
                let style = if history_active && item_index == app.history_selected {
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

    let history_list = List::new(history_items).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(if history_active {
                Style::default().fg(Color::Green)
            } else {
                Style::default().fg(Color::White)
            })
            .title("History (j/k/l/Enter)"),
    );

    f.render_widget(history_list, bottom_chunks[1]);
}

fn render_input(f: &mut Frame, area: Rect, app: &App) {
    let input_active = app.active_pane == ActivePane::Input;
    let text = format!("> {}", app.tx_input);
    let paragraph = Paragraph::new(text)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(if input_active {
                    Style::default().fg(Color::Green)
                } else {
                    Style::default().fg(Color::White)
                })
                .title("Input"),
        )
        .style(Style::default().fg(Color::Cyan));
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
    let clear_block = Block::default()
        .style(Style::default().bg(Color::Black));
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

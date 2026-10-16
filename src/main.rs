use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::ExecutableCommand;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph};
use ratatui::Terminal;
use std::fs;
use std::io::stdout;
use std::path::PathBuf;

// ── 数据类型定义 ──────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq)]
enum DataType {
    U8,
    U16,
    U32,
    F32,
    F64,
}

impl DataType {
    fn all() -> &'static [DataType] {
        &[DataType::U8, DataType::U16, DataType::U32, DataType::F32, DataType::F64]
    }

    fn size(&self) -> usize {
        match self {
            DataType::U8 => 1,
            DataType::U16 => 2,
            DataType::U32 | DataType::F32 => 4,
            DataType::F64 => 8,
        }
    }

    fn label(&self) -> &'static str {
        match self {
            DataType::U8 => "u8  (1 byte)",
            DataType::U16 => "u16 (2 bytes)",
            DataType::U32 => "u32 (4 bytes)",
            DataType::F32 => "f32 (4 bytes)",
            DataType::F64 => "f64 (8 bytes)",
        }
    }
}

// ── 核心逻辑：数据拆分与交叉重组 ──────────────────────────────────────────────

/// 将数据按指定类型大小拆分为 N 组，然后按 1,2,3..N 顺序交叉合并
fn interleave(data: &[u8], n: usize, dtype: DataType) -> Result<Vec<u8>, String> {
    let elem_size = dtype.size();
    let total_elems = data.len() / elem_size;

    if total_elems == 0 {
        return Err("文件数据不足以容纳任何完整元素".into());
    }
    if n == 0 {
        return Err("分组数 N 必须大于 0".into());
    }
    if n == 1 {
        return Ok(data.to_vec());
    }

    // 每组元素数（向上取整，最后一组可能不足）
    let base_count = total_elems / n;
    let remainder = total_elems % n;

    // 构建每组的元素数量
    let group_sizes: Vec<usize> = (0..n)
        .map(|i| if i < remainder { base_count + 1 } else { base_count })
        .collect();

    // 构建每组的起始偏移（以元素为单位）
    let mut group_starts = Vec::with_capacity(n);
    let mut offset = 0usize;
    for &sz in &group_sizes {
        group_starts.push(offset);
        offset += sz;
    }

    let max_count = group_sizes.iter().copied().max().unwrap_or(0);
    let mut output = Vec::with_capacity(data.len());

    // 按元素位置交叉：pos 0..max_count，每组依次取出
    for pos in 0..max_count {
        for grp in 0..n {
            if pos < group_sizes[grp] {
                let elem_idx = group_starts[grp] + pos;
                let byte_start = elem_idx * elem_size;
                let byte_end = byte_start + elem_size;
                if byte_end <= data.len() {
                    output.extend_from_slice(&data[byte_start..byte_end]);
                }
            }
        }
    }

    // 保留末尾不足一个元素的尾部字节（原样追加）
    let used = total_elems * elem_size;
    if used < data.len() {
        output.extend_from_slice(&data[used..]);
    }

    Ok(output)
}

// ── TUI 状态 ──────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq)]
enum FocusField {
    FilePath,
    NumGroups,
    DataType,
    Action,
}

struct AppState {
    file_path: String,
    num_groups_str: String,
    dtype_index: usize,
    focus: FocusField,
    dtype_open: bool,
    message: Option<String>,
    message_is_error: bool,
    should_quit: bool,
}

impl AppState {
    fn new() -> Self {
        Self {
            file_path: String::new(),
            num_groups_str: "2".into(),
            dtype_index: 0,
            focus: FocusField::FilePath,
            dtype_open: false,
            message: None,
            message_is_error: false,
            should_quit: false,
        }
    }

    fn current_dtype(&self) -> DataType {
        DataType::all()[self.dtype_index]
    }
}

// ── 执行操作 ──────────────────────────────────────────────────────────────────

fn run_interleave(state: &mut AppState) {
    // 校验路径
    let path = PathBuf::from(&state.file_path);
    if !path.exists() {
        state.message = Some(format!("文件不存在: {}", state.file_path));
        state.message_is_error = true;
        return;
    }

    // 读取文件
    let data = match fs::read(&path) {
        Ok(d) => d,
        Err(e) => {
            state.message = Some(format!("读取文件失败: {e}"));
            state.message_is_error = true;
            return;
        }
    };

    let file_size = data.len();

    // 解析 N
    let n: usize = match state.num_groups_str.parse() {
        Ok(v) if v > 0 => v,
        _ => {
            state.message = Some("分组数必须为正整数".into());
            state.message_is_error = true;
            return;
        }
    };

    let dtype = state.current_dtype();

    // 执行交叉
    match interleave(&data, n, dtype) {
        Ok(output) => {
            // 生成输出路径：原文件名_interleaved_N_dtype.ext
            let stem = path.file_stem().unwrap_or_default().to_string_lossy();
            let ext = path.extension().map(|e| e.to_string_lossy().to_string()).unwrap_or_default();
            let dtype_name = match dtype {
                DataType::U8 => "u8",
                DataType::U16 => "u16",
                DataType::U32 => "u32",
                DataType::F32 => "f32",
                DataType::F64 => "f64",
            };
            let out_name = if ext.is_empty() {
                format!("{stem}_interleaved_{n}_{dtype_name}")
            } else {
                format!("{stem}_interleaved_{n}_{dtype_name}.{ext}")
            };
            let out_path = path.with_file_name(&out_name);

            match fs::write(&out_path, &output) {
                Ok(_) => {
                    state.message = Some(format!(
                        "✓ 完成! 文件: {} | 大小: {} bytes | 分组: {} | 类型: {} | 输出: {}",
                        stem, file_size, n, dtype.label(), out_path.display()
                    ));
                    state.message_is_error = false;
                }
                Err(e) => {
                    state.message = Some(format!("写入失败: {e}"));
                    state.message_is_error = true;
                }
            }
        }
        Err(e) => {
            state.message = Some(format!("处理失败: {e}"));
            state.message_is_error = true;
        }
    }
}

// ── 键盘事件处理 ──────────────────────────────────────────────────────────────

fn handle_key(state: &mut AppState, key: KeyCode) {
    // 全局退出
    if key == KeyCode::Char('q') && !matches!(state.focus, FocusField::FilePath | FocusField::NumGroups) {
        state.should_quit = true;
        return;
    }
    if key == KeyCode::Esc {
        if state.dtype_open {
            state.dtype_open = false;
            return;
        }
        state.should_quit = true;
        return;
    }

    // 数据类型下拉展开时，优先处理上下选择
    if state.dtype_open {
        match key {
            KeyCode::Up => {
                if state.dtype_index > 0 {
                    state.dtype_index -= 1;
                }
                return;
            }
            KeyCode::Down => {
                if state.dtype_index < DataType::all().len() - 1 {
                    state.dtype_index += 1;
                }
                return;
            }
            KeyCode::Enter => {
                state.dtype_open = false;
                return;
            }
            _ => {
                state.dtype_open = false;
                // fall through
            }
        }
    }

    match state.focus {
        FocusField::FilePath => match key {
            KeyCode::Enter | KeyCode::Down | KeyCode::Tab => state.focus = FocusField::NumGroups,
            KeyCode::Backspace => { state.file_path.pop(); }
            KeyCode::Char(c) => state.file_path.push(c),
            _ => {}
        },
        FocusField::NumGroups => match key {
            KeyCode::Enter | KeyCode::Down | KeyCode::Tab => state.focus = FocusField::DataType,
            KeyCode::Up => state.focus = FocusField::FilePath,
            KeyCode::Backspace => { state.num_groups_str.pop(); }
            KeyCode::Char(c) if c.is_ascii_digit() => state.num_groups_str.push(c),
            _ => {}
        },
        FocusField::DataType => match key {
            KeyCode::Enter | KeyCode::Char(' ') | KeyCode::Down | KeyCode::Tab => {
                state.dtype_open = true;
            }
            KeyCode::Up => state.focus = FocusField::NumGroups,
            KeyCode::Left => {
                if state.dtype_index > 0 { state.dtype_index -= 1; }
            }
            KeyCode::Right => {
                if state.dtype_index < DataType::all().len() - 1 { state.dtype_index += 1; }
            }
            _ => {}
        },
        FocusField::Action => match key {
            KeyCode::Up => state.focus = FocusField::DataType,
            KeyCode::Enter => run_interleave(state),
            _ => {}
        },
    }
}

// ── 渲染 ──────────────────────────────────────────────────────────────────────

fn draw(f: &mut ratatui::Frame, state: &AppState) {
    let area = f.area();

    // 背景
    let bg = Block::default()
        .style(Style::default().bg(Color::Rgb(24, 24, 28)));
    f.render_widget(bg, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),  // 标题
            Constraint::Length(3),  // 文件路径
            Constraint::Length(3),  // 分组数
            Constraint::Length(3),  // 数据类型
            Constraint::Length(3),  // 执行按钮
            Constraint::Min(3),    // 消息区
            Constraint::Length(3), // 帮助栏
        ])
        .split(area);

    // ── 标题 ──
    let title = Paragraph::new(Line::from(vec![
        Span::styled(" BinTools ", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
        Span::raw("— 二进制文件数据重组工具"),
    ]))
    .block(Block::default().borders(Borders::BOTTOM).border_style(Style::default().fg(Color::DarkGray)));
    f.render_widget(title, chunks[0]);

    // ── 文件路径 ──
    let fp_focused = state.focus == FocusField::FilePath;
    let fp_style = if fp_focused {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default().fg(Color::White)
    };
    let cursor = if fp_focused { "▌" } else { "" };
    let fp_text = if state.file_path.is_empty() && !fp_focused {
        Span::styled("请输入文件路径...", Style::default().fg(Color::DarkGray))
    } else {
        Span::styled(format!("{}{}", state.file_path, cursor), fp_style)
    };
    let fp_block = Block::default()
        .borders(Borders::ALL)
        .border_style(if fp_focused { Style::default().fg(Color::Yellow) } else { Style::default().fg(Color::DarkGray) })
        .title(" 文件路径 (Enter/↓ 下一步) ");
    let fp_widget = Paragraph::new(Line::from(vec![fp_text])).block(fp_block);
    f.render_widget(fp_widget, chunks[1]);

    // ── 分组数 N ──
    let ng_focused = state.focus == FocusField::NumGroups;
    let ng_style = if ng_focused { Style::default().fg(Color::Green) } else { Style::default().fg(Color::White) };
    let ng_cursor = if ng_focused { "▌" } else { "" };
    let ng_block = Block::default()
        .borders(Borders::ALL)
        .border_style(if ng_focused { Style::default().fg(Color::Green) } else { Style::default().fg(Color::DarkGray) })
        .title(" 分组数 N (仅数字) ");
    let ng_widget = Paragraph::new(Line::from(vec![
        Span::styled(format!("{}{}", state.num_groups_str, ng_cursor), ng_style)
    ])).block(ng_block);
    f.render_widget(ng_widget, chunks[2]);

    // ── 数据类型 ──
    let dt_focused = state.focus == FocusField::DataType;
    let dt_block = Block::default()
        .borders(Borders::ALL)
        .border_style(if dt_focused || state.dtype_open {
            Style::default().fg(Color::Magenta)
        } else {
            Style::default().fg(Color::DarkGray)
        })
        .title(" 数据块类型 (← → 切换 / Enter 展开列表) ");
    let dt_label = Span::styled(
        state.current_dtype().label(),
        if dt_focused { Style::default().fg(Color::Magenta) } else { Style::default().fg(Color::White) },
    );
    let dt_widget = Paragraph::new(Line::from(vec![dt_label])).block(dt_block);
    f.render_widget(dt_widget, chunks[3]);

    // 数据类型下拉列表
    if state.dtype_open {
        let popup_area = centered_rect(30, 7, area);
        f.render_widget(Clear, popup_area);
        let items: Vec<ListItem> = DataType::all()
            .iter()
            .enumerate()
            .map(|(i, dt)| {
                let style = if i == state.dtype_index {
                    Style::default().fg(Color::Black).bg(Color::Magenta)
                } else {
                    Style::default().fg(Color::White)
                };
                ListItem::new(dt.label()).style(style)
            })
            .collect();
        let list = List::new(items)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::Magenta))
                    .title(" 选择数据类型 (↑↓ 选择, Enter 确认) ")
                    .style(Style::default().bg(Color::Rgb(32, 32, 38))),
            );
        f.render_widget(list, popup_area);
    }

    // ── 执行按钮 ──
    let action_focused = state.focus == FocusField::Action;
    let action_style = if action_focused {
        Style::default().fg(Color::Black).bg(Color::Cyan).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::Cyan)
    };
    let action_block = Block::default()
        .borders(Borders::ALL)
        .border_style(if action_focused { Style::default().fg(Color::Cyan) } else { Style::default().fg(Color::DarkGray) })
        .title(" 执行 ");
    let action_text = if action_focused { " ▶ 执行数据重组 (Enter) " } else { "   执行数据重组   "};
    let action_widget = Paragraph::new(Line::from(vec![
        Span::styled(action_text, action_style)
    ])).block(action_block);
    f.render_widget(action_widget, chunks[4]);

    // ── 消息区 ──
    let msg_style = if state.message_is_error {
        Style::default().fg(Color::Red)
    } else {
        Style::default().fg(Color::Green)
    };
    let msg_text = state.message.as_deref().unwrap_or("就绪 — 填写参数后按 Enter 执行");
    let msg_style_final = if state.message.is_none() {
        Style::default().fg(Color::DarkGray)
    } else {
        msg_style
    };
    let msg_block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(" 状态 ");
    let msg_widget = Paragraph::new(Line::from(vec![Span::styled(msg_text, msg_style_final)]))
        .block(msg_block);
    f.render_widget(msg_widget, chunks[5]);

    // ── 帮助栏 ──
    let help = Paragraph::new(Line::from(vec![
        Span::styled(" Tab/↓ ", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
        Span::styled("下一字段  ", Style::default().fg(Color::DarkGray)),
        Span::styled(" ↑ ", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
        Span::styled("上一字段  ", Style::default().fg(Color::DarkGray)),
        Span::styled(" Enter ", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
        Span::styled("确认/执行  ", Style::default().fg(Color::DarkGray)),
        Span::styled(" q/Esc ", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
        Span::styled("退出", Style::default().fg(Color::DarkGray)),
    ]))
    .block(Block::default().borders(Borders::TOP).border_style(Style::default().fg(Color::DarkGray)));
    f.render_widget(help, chunks[6]);
}

/// 居中矩形辅助
fn centered_rect(width: u16, height: u16, area: Rect) -> Rect {
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    Rect::new(x, y, width.min(area.width), height.min(area.height))
}

// ── 主循环 ────────────────────────────────────────────────────────────────────

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 初始化终端
    enable_raw_mode()?;
    stdout().execute(EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout());
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;

    let mut state = AppState::new();

    // 如果传入了命令行参数作为文件路径，预填
    if let Some(arg) = std::env::args().nth(1) {
        state.file_path = arg;
        state.focus = FocusField::NumGroups;
    }

    loop {
        terminal.draw(|f| draw(f, &state))?;

        if let Event::Key(key) = event::read()? {
            if key.kind != KeyEventKind::Press {
                continue;
            }
            handle_key(&mut state, key.code);
        }

        if state.should_quit {
            break;
        }
    }

    // 恢复终端
    disable_raw_mode()?;
    stdout().execute(LeaveAlternateScreen)?;
    Ok(())
}

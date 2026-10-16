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

    fn short_name(&self) -> &'static str {
        match self {
            DataType::U8 => "u8",
            DataType::U16 => "u16",
            DataType::U32 => "u32",
            DataType::F32 => "f32",
            DataType::F64 => "f64",
        }
    }
}

// ── 操作模式 ──────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq)]
enum OpMode {
    /// 顺序 → 交错 (111222 → 121212)
    SeqToInt,
    /// 交错 → 顺序 (121212 → 111222)
    IntToSeq,
    /// 顺序拆分文件
    SplitSeq,
    /// 交错拆分文件
    SplitInt,
    /// 顺序合并文件 (多个文件 → 111222)
    MergeSeq,
    /// 交错合并文件 (多个文件 → 121212)
    MergeInt,
}

impl OpMode {
    fn all() -> &'static [OpMode] {
        &[OpMode::SeqToInt, OpMode::IntToSeq, OpMode::SplitSeq, OpMode::SplitInt, OpMode::MergeSeq, OpMode::MergeInt]
    }

    fn label(&self) -> &'static str {
        match self {
            OpMode::SeqToInt => "顺序 → 交错  (111222 → 121212)",
            OpMode::IntToSeq => "交错 → 顺序  (121212 → 111222)",
            OpMode::SplitSeq => "顺序拆分文件  (111222 → 多个文件)",
            OpMode::SplitInt => "交错拆分文件  (121212 → 多个文件)",
            OpMode::MergeSeq => "顺序合并文件  (多个文件 → 111222)",
            OpMode::MergeInt => "交错合并文件  (多个文件 → 121212)",
        }
    }

    fn short_label(&self) -> &'static str {
        match self {
            OpMode::SeqToInt => "顺序→交错",
            OpMode::IntToSeq => "交错→顺序",
            OpMode::SplitSeq => "顺序拆分",
            OpMode::SplitInt => "交错拆分",
            OpMode::MergeSeq => "顺序合并",
            OpMode::MergeInt => "交错合并",
        }
    }

    /// 是否为合并模式
    fn is_merge(&self) -> bool {
        matches!(self, OpMode::MergeSeq | OpMode::MergeInt)
    }
}

// ── 核心逻辑 ──────────────────────────────────────────────────────────────────

/// 顺序 → 交错: 输入为 111222 格式，输出为 121212 格式
/// 将数据按 N 组顺序排列，拆分为 N 段，然后交叉合并
fn seq_to_int(data: &[u8], n: usize, dtype: DataType) -> Result<Vec<u8>, String> {
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

    // 顺序格式: 组0 组1 ... 组N-1，每组元素数
    let base_count = total_elems / n;
    let remainder = total_elems % n;

    let group_sizes: Vec<usize> = (0..n)
        .map(|i| if i < remainder { base_count + 1 } else { base_count })
        .collect();

    let mut group_starts = Vec::with_capacity(n);
    let mut offset = 0usize;
    for &sz in &group_sizes {
        group_starts.push(offset);
        offset += sz;
    }

    let max_count = group_sizes.iter().copied().max().unwrap_or(0);
    let mut output = Vec::with_capacity(data.len());

    // 交叉输出: 每个位置从每组取一个元素
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

    // 尾部不足一个元素的字节原样保留
    let used = total_elems * elem_size;
    if used < data.len() {
        output.extend_from_slice(&data[used..]);
    }

    Ok(output)
}

/// 交错 → 顺序: 输入为 121212 格式，输出为 111222 格式
/// 将交错数据还原为按组顺序排列
fn int_to_seq(data: &[u8], n: usize, dtype: DataType) -> Result<Vec<u8>, String> {
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

    // 交错格式中，每组元素数与 seq_to_int 相同
    let base_count = total_elems / n;
    let remainder = total_elems % n;

    let group_sizes: Vec<usize> = (0..n)
        .map(|i| if i < remainder { base_count + 1 } else { base_count })
        .collect();

    let max_count = group_sizes.iter().copied().max().unwrap_or(0);

    // 先构建交错格式中的索引映射
    // 交错输出中第 k 个元素属于哪个组、哪个位置
    let mut output = vec![0u8; total_elems * elem_size];

    let mut out_pos = 0usize; // 交错数据中的元素序号
    for pos in 0..max_count {
        for grp in 0..n {
            if pos < group_sizes[grp] {
                // 交错数据中第 out_pos 个元素 → 顺序数据中组 grp 的第 pos 个元素
                // 计算组 grp 在顺序输出中的起始元素索引
                let seq_start: usize = group_sizes[..grp].iter().sum();
                let seq_elem_idx = seq_start + pos;
                let dst_byte = seq_elem_idx * elem_size;
                let src_byte = out_pos * elem_size;
                if src_byte + elem_size <= data.len() && dst_byte + elem_size <= output.len() {
                    output[dst_byte..dst_byte + elem_size]
                        .copy_from_slice(&data[src_byte..src_byte + elem_size]);
                }
                out_pos += 1;
            }
        }
    }

    // 尾部字节
    let used = total_elems * elem_size;
    if used < data.len() {
        output.extend_from_slice(&data[used..]);
    }

    Ok(output)
}

/// 顺序拆分: 将顺序排列的数据拆分为 N 个独立文件
fn split_seq(data: &[u8], n: usize, dtype: DataType) -> Result<Vec<Vec<u8>>, String> {
    let elem_size = dtype.size();
    let total_elems = data.len() / elem_size;

    if total_elems == 0 {
        return Err("文件数据不足以容纳任何完整元素".into());
    }
    if n == 0 {
        return Err("分组数 N 必须大于 0".into());
    }

    let base_count = total_elems / n;
    let remainder = total_elems % n;

    let group_sizes: Vec<usize> = (0..n)
        .map(|i| if i < remainder { base_count + 1 } else { base_count })
        .collect();

    let mut results = Vec::with_capacity(n);
    let mut elem_offset = 0usize;
    for &count in &group_sizes {
        let byte_start = elem_offset * elem_size;
        let byte_end = byte_start + count * elem_size;
        let chunk = if byte_end <= data.len() {
            data[byte_start..byte_end].to_vec()
        } else {
            data[byte_start..].to_vec()
        };
        results.push(chunk);
        elem_offset += count;
    }

    Ok(results)
}

/// 交错拆分: 将交错排列的数据拆分为 N 个独立文件
fn split_int(data: &[u8], n: usize, dtype: DataType) -> Result<Vec<Vec<u8>>, String> {
    let elem_size = dtype.size();
    let total_elems = data.len() / elem_size;

    if total_elems == 0 {
        return Err("文件数据不足以容纳任何完整元素".into());
    }
    if n == 0 {
        return Err("分组数 N 必须大于 0".into());
    }

    let base_count = total_elems / n;
    let remainder = total_elems % n;

    let group_sizes: Vec<usize> = (0..n)
        .map(|i| if i < remainder { base_count + 1 } else { base_count })
        .collect();

    let max_count = group_sizes.iter().copied().max().unwrap_or(0);

    let mut groups: Vec<Vec<u8>> = (0..n).map(|_| Vec::new()).collect();

    let mut src_pos = 0usize;
    for pos in 0..max_count {
        for grp in 0..n {
            if pos < group_sizes[grp] {
                let byte_start = src_pos * elem_size;
                let byte_end = byte_start + elem_size;
                if byte_end <= data.len() {
                    groups[grp].extend_from_slice(&data[byte_start..byte_end]);
                }
                src_pos += 1;
            }
        }
    }

    Ok(groups)
}

// ── 文件发现与合并 ──────────────────────────────────────────────────────────────

/// 顺序合并: 将多个文件按顺序拼接 (111 + 222 + 333 → 111222333)
fn merge_seq(file_data: &[Vec<u8>]) -> Vec<u8> {
    let total: usize = file_data.iter().map(|d| d.len()).sum();
    let mut output = Vec::with_capacity(total);
    for data in file_data {
        output.extend_from_slice(data);
    }
    output
}

/// 交错合并: 将多个文件按元素交错合并 (1+2+3 → 121212)
fn merge_int(file_data: &[Vec<u8>], dtype: DataType) -> Result<Vec<u8>, String> {
    let elem_size = dtype.size();

    // 计算每组的元素数
    let group_elems: Vec<usize> = file_data
        .iter()
        .map(|d| d.len() / elem_size)
        .collect();
    let max_elems = group_elems.iter().copied().max().unwrap_or(0);

    let mut output = Vec::new();

    for pos in 0..max_elems {
        for (grp, data) in file_data.iter().enumerate() {
            if pos < group_elems[grp] {
                let byte_start = pos * elem_size;
                let byte_end = byte_start + elem_size;
                if byte_end <= data.len() {
                    output.extend_from_slice(&data[byte_start..byte_end]);
                }
            }
        }
    }

    Ok(output)
}

// ── TUI 状态 ──────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq)]
enum FocusField {
    Mode,
    FilePath,
    FileList,
    NumGroups,
    DataType,
    Action,
}

struct AppState {
    mode_index: usize,
    file_path: String,
    merge_files: Vec<String>,
    selected_file: usize,
    num_groups_str: String,
    dtype_index: usize,
    focus: FocusField,
    mode_open: bool,
    dtype_open: bool,
    message: Option<String>,
    message_is_error: bool,
    should_quit: bool,
}

impl AppState {
    fn new() -> Self {
        Self {
            mode_index: 0,
            file_path: String::new(),
            merge_files: Vec::new(),
            selected_file: 0,
            num_groups_str: "2".into(),
            dtype_index: 0,
            focus: FocusField::Mode,
            mode_open: false,
            dtype_open: false,
            message: None,
            message_is_error: false,
            should_quit: false,
        }
    }

    fn current_mode(&self) -> OpMode {
        OpMode::all()[self.mode_index]
    }

    fn current_dtype(&self) -> DataType {
        DataType::all()[self.dtype_index]
    }
}

// ── 文件输出辅助 ──────────────────────────────────────────────────────────────

fn make_output_path(path: &PathBuf, suffix: &str) -> PathBuf {
    let stem = path.file_stem().unwrap_or_default().to_string_lossy();
    let ext = path.extension().map(|e| e.to_string_lossy().to_string()).unwrap_or_default();
    let out_name = if ext.is_empty() {
        format!("{stem}_{suffix}")
    } else {
        format!("{stem}_{suffix}.{ext}")
    };
    path.with_file_name(&out_name)
}

fn split_suffix(index: usize) -> String {
    format!("{:03}", index + 1)
}

// ── 执行操作 ──────────────────────────────────────────────────────────────────

fn run_action(state: &mut AppState) {
    let mode = state.current_mode();
    let path = PathBuf::from(&state.file_path);

    // ── 合并模式：使用文件列表 ──
    if mode.is_merge() {
        if state.merge_files.len() < 2 {
            state.message = Some("合并至少需要 2 个文件".into());
            state.message_is_error = true;
            return;
        }

        // 验证所有文件存在
        for fp in &state.merge_files {
            if !PathBuf::from(fp).exists() {
                state.message = Some(format!("文件不存在: {}", fp));
                state.message_is_error = true;
                return;
            }
        }

        let files: Vec<PathBuf> = state.merge_files.iter().map(PathBuf::from).collect();
        let dtype = state.current_dtype();

        // 读取所有文件
        let mut file_data: Vec<Vec<u8>> = Vec::new();
        for f in &files {
            match fs::read(f) {
                Ok(d) => file_data.push(d),
                Err(e) => {
                    state.message = Some(format!("读取 {} 失败: {e}", f.display()));
                    state.message_is_error = true;
                    return;
                }
            }
        }

        let total_input: usize = file_data.iter().map(|d| d.len()).sum();
        let file_count = files.len();

        let output = match mode {
            OpMode::MergeSeq => merge_seq(&file_data),
            OpMode::MergeInt => match merge_int(&file_data, dtype) {
                Ok(d) => d,
                Err(e) => {
                    state.message = Some(format!("处理失败: {e}"));
                    state.message_is_error = true;
                    return;
                }
            },
            _ => unreachable!(),
        };

        // 输出文件: 去掉 _NNN 后缀，加上合并标记
        let first_file = &files[0];
        let stem = first_file.file_stem().unwrap_or_default().to_string_lossy().to_string();
        // 去掉 _NNN 后缀
        let base_stem = if let Some(us_pos) = stem.rfind('_') {
            &stem[..us_pos]
        } else {
            &stem
        };
        let ext = first_file.extension().map(|e| e.to_string_lossy().to_string()).unwrap_or_default();
        let tag = match mode {
            OpMode::MergeSeq => "merged_seq",
            OpMode::MergeInt => "merged_int",
            _ => unreachable!(),
        };
        let out_name = if ext.is_empty() {
            format!("{base_stem}_{tag}")
        } else {
            format!("{base_stem}_{tag}.{ext}")
        };
        let out_path = first_file.with_file_name(&out_name);

        match fs::write(&out_path, &output) {
            Ok(_) => {
                let mode_desc = match mode {
                    OpMode::MergeSeq => "顺序合并",
                    OpMode::MergeInt => "交错合并",
                    _ => unreachable!(),
                };
                state.message = Some(format!(
                    "✓ {} 完成! {} 个文件 ({} bytes) → {} bytes | 输出: {}",
                    mode_desc, file_count, total_input, output.len(), out_path.display()
                ));
                state.message_is_error = false;
            }
            Err(e) => {
                state.message = Some(format!("写入失败: {e}"));
                state.message_is_error = true;
            }
        }
        return;
    }

    // ── 非合并模式：单文件操作 ──
    if !path.exists() {
        state.message = Some(format!("文件不存在: {}", state.file_path));
        state.message_is_error = true;
        return;
    }

    let data = match fs::read(&path) {
        Ok(d) => d,
        Err(e) => {
            state.message = Some(format!("读取文件失败: {e}"));
            state.message_is_error = true;
            return;
        }
    };

    let file_size = data.len();

    let n: usize = match state.num_groups_str.parse() {
        Ok(v) if v > 0 => v,
        _ => {
            state.message = Some("分组数必须为正整数".into());
            state.message_is_error = true;
            return;
        }
    };

    let dtype = state.current_dtype();
    let stem = path.file_stem().unwrap_or_default().to_string_lossy().to_string();

    match mode {
        OpMode::SeqToInt => {
            match seq_to_int(&data, n, dtype) {
                Ok(output) => {
                    let suffix = format!("s2i_{}_{}", n, dtype.short_name());
                    let out_path = make_output_path(&path, &suffix);
                    match fs::write(&out_path, &output) {
                        Ok(_) => {
                            state.message = Some(format!(
                                "✓ 顺序→交错 完成! {} bytes | N={} | {} | 输出: {}",
                                file_size, n, dtype.label(), out_path.display()
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
        OpMode::IntToSeq => {
            match int_to_seq(&data, n, dtype) {
                Ok(output) => {
                    let suffix = format!("i2s_{}_{}", n, dtype.short_name());
                    let out_path = make_output_path(&path, &suffix);
                    match fs::write(&out_path, &output) {
                        Ok(_) => {
                            state.message = Some(format!(
                                "✓ 交错→顺序 完成! {} bytes | N={} | {} | 输出: {}",
                                file_size, n, dtype.label(), out_path.display()
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
        OpMode::SplitSeq => {
            match split_seq(&data, n, dtype) {
                Ok(chunks) => {
                    let mut written = 0usize;
                    for (i, chunk) in chunks.iter().enumerate() {
                        let suffix = format!("split_{}", split_suffix(i));
                        let out_path = make_output_path(&path, &suffix);
                        if let Err(e) = fs::write(&out_path, chunk) {
                            state.message = Some(format!("写入文件 {} 失败: {e}", out_path.display()));
                            state.message_is_error = true;
                            return;
                        }
                        written += chunk.len();
                    }
                    state.message = Some(format!(
                        "✓ 顺序拆分 完成! {} bytes → {} 个文件 | N={} | {}",
                        file_size, chunks.len(), n, stem
                    ));
                    state.message_is_error = false;
                    let _ = written;
                }
                Err(e) => {
                    state.message = Some(format!("处理失败: {e}"));
                    state.message_is_error = true;
                }
            }
        }
        OpMode::SplitInt => {
            match split_int(&data, n, dtype) {
                Ok(chunks) => {
                    let mut written = 0usize;
                    for (i, chunk) in chunks.iter().enumerate() {
                        let suffix = format!("split_{}", split_suffix(i));
                        let out_path = make_output_path(&path, &suffix);
                        if let Err(e) = fs::write(&out_path, chunk) {
                            state.message = Some(format!("写入文件 {} 失败: {e}", out_path.display()));
                            state.message_is_error = true;
                            return;
                        }
                        written += chunk.len();
                    }
                    state.message = Some(format!(
                        "✓ 交错拆分 完成! {} bytes → {} 个文件 | N={} | {}",
                        file_size, chunks.len(), n, stem
                    ));
                    state.message_is_error = false;
                    let _ = written;
                }
                Err(e) => {
                    state.message = Some(format!("处理失败: {e}"));
                    state.message_is_error = true;
                }
            }
        }
        // 合并模式已在上方提前返回，此处不会到达
        _ => {}
    }
}

// ── 键盘事件处理 ──────────────────────────────────────────────────────────────

fn handle_key(state: &mut AppState, key: KeyCode) {
    // 模式选择下拉时
    if state.mode_open {
        match key {
            KeyCode::Up => {
                if state.mode_index > 0 { state.mode_index -= 1; }
                return;
            }
            KeyCode::Down => {
                if state.mode_index < OpMode::all().len() - 1 { state.mode_index += 1; }
                return;
            }
            KeyCode::Enter => {
                state.mode_open = false;
                return;
            }
            KeyCode::Esc => {
                state.mode_open = false;
                return;
            }
            _ => {
                state.mode_open = false;
                // fall through
            }
        }
    }

    // 数据类型下拉展开时
    if state.dtype_open {
        match key {
            KeyCode::Up => {
                if state.dtype_index > 0 { state.dtype_index -= 1; }
                return;
            }
            KeyCode::Down => {
                if state.dtype_index < DataType::all().len() - 1 { state.dtype_index += 1; }
                return;
            }
            KeyCode::Enter => {
                state.dtype_open = false;
                return;
            }
            KeyCode::Esc => {
                state.dtype_open = false;
                return;
            }
            _ => {
                state.dtype_open = false;
                // fall through
            }
        }
    }

    // 全局退出
    if key == KeyCode::Char('q') && !matches!(state.focus, FocusField::FilePath | FocusField::NumGroups) {
        state.should_quit = true;
        return;
    }
    if key == KeyCode::Esc {
        state.should_quit = true;
        return;
    }

    match state.focus {
        FocusField::Mode => match key {
            KeyCode::Enter | KeyCode::Char(' ') => {
                state.mode_open = true;
            }
            KeyCode::Down | KeyCode::Tab => state.focus = FocusField::FilePath,
            KeyCode::Up => state.focus = FocusField::Action,
            KeyCode::Left => {
                if state.mode_index > 0 { state.mode_index -= 1; }
            }
            KeyCode::Right => {
                if state.mode_index < OpMode::all().len() - 1 { state.mode_index += 1; }
            }
            _ => {}
        },
        FocusField::FilePath => match key {
            KeyCode::Enter => {
                if state.current_mode().is_merge() {
                    // 合并模式: Enter 添加文件到列表
                    let p = state.file_path.trim().to_string();
                    if !p.is_empty() {
                        if PathBuf::from(&p).exists() {
                            state.merge_files.push(p);
                            state.file_path.clear();
                            state.selected_file = state.merge_files.len().saturating_sub(1);
                            state.message = Some(format!("✓ 已添加文件，当前列表 {} 个文件", state.merge_files.len()));
                            state.message_is_error = false;
                        } else {
                            state.message = Some(format!("文件不存在: {}", p));
                            state.message_is_error = true;
                        }
                    }
                } else {
                    state.focus = FocusField::NumGroups;
                }
            }
            KeyCode::Down | KeyCode::Tab => {
                state.focus = if state.current_mode().is_merge() {
                    FocusField::FileList
                } else {
                    FocusField::NumGroups
                };
            }
            KeyCode::Up => state.focus = FocusField::Mode,
            KeyCode::Backspace => { state.file_path.pop(); }
            KeyCode::Char(c) => state.file_path.push(c),
            _ => {}
        },
        FocusField::FileList => match key {
            KeyCode::Up => {
                if state.merge_files.is_empty() || state.selected_file == 0 {
                    state.focus = FocusField::FilePath;
                } else {
                    state.selected_file -= 1;
                }
            }
            KeyCode::Down | KeyCode::Tab => state.focus = FocusField::DataType,
            KeyCode::Delete | KeyCode::Backspace => {
                if !state.merge_files.is_empty() && state.selected_file < state.merge_files.len() {
                    state.merge_files.remove(state.selected_file);
                    if state.selected_file >= state.merge_files.len() && state.selected_file > 0 {
                        state.selected_file -= 1;
                    }
                }
            }
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
            KeyCode::Enter | KeyCode::Char(' ') => {
                state.dtype_open = true;
            }
            KeyCode::Down | KeyCode::Tab => state.focus = FocusField::Action,
            KeyCode::Up => {
                state.focus = if state.current_mode().is_merge() {
                    FocusField::FileList
                } else {
                    FocusField::NumGroups
                };
            }
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
            KeyCode::Down | KeyCode::Tab => state.focus = FocusField::Mode,
            KeyCode::Enter => run_action(state),
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

    let is_merge = state.current_mode().is_merge();

    // 动态布局: 合并模式多一个文件列表区域
    let chunks = if is_merge {
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),  // 标题
                Constraint::Length(3),  // 操作模式
                Constraint::Length(3),  // 添加文件输入
                Constraint::Min(5),    // 文件列表
                Constraint::Length(3),  // 数据类型
                Constraint::Length(3),  // 执行按钮
                Constraint::Min(3),    // 消息区
                Constraint::Length(3), // 帮助栏
            ])
            .split(area)
    } else {
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),  // 标题
                Constraint::Length(3),  // 操作模式
                Constraint::Length(3),  // 文件路径
                Constraint::Length(3),  // 分组数
                Constraint::Length(3),  // 数据类型
                Constraint::Length(3),  // 执行按钮
                Constraint::Min(3),    // 消息区
                Constraint::Length(3), // 帮助栏
            ])
            .split(area)
    };

    // ── 标题 ──
    let title = Paragraph::new(Line::from(vec![
        Span::styled(" BinTools ", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
        Span::raw("— 二进制文件数据重组工具"),
    ]))
    .block(Block::default().borders(Borders::BOTTOM).border_style(Style::default().fg(Color::DarkGray)));
    f.render_widget(title, chunks[0]);

    // ── 操作模式 ──
    let mode_focused = state.focus == FocusField::Mode;
    let mode_block = Block::default()
        .borders(Borders::ALL)
        .border_style(if mode_focused || state.mode_open {
            Style::default().fg(Color::Cyan)
        } else {
            Style::default().fg(Color::DarkGray)
        })
        .title(" 操作模式 (← → 切换 / Enter 选择列表) ");
    let mode_label = Span::styled(
        state.current_mode().label(),
        if mode_focused {
            Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White)
        },
    );
    let mode_widget = Paragraph::new(Line::from(vec![mode_label])).block(mode_block);
    f.render_widget(mode_widget, chunks[1]);

    // 模式下拉列表
    if state.mode_open {
        let popup_area = centered_rect(42, 8, area);
        f.render_widget(Clear, popup_area);
        let items: Vec<ListItem> = OpMode::all()
            .iter()
            .enumerate()
            .map(|(i, m)| {
                let style = if i == state.mode_index {
                    Style::default().fg(Color::Black).bg(Color::Cyan)
                } else {
                    Style::default().fg(Color::White)
                };
                ListItem::new(m.label()).style(style)
            })
            .collect();
        let list = List::new(items)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::Cyan))
                    .title(" 选择操作模式 (↑↓ 选择, Enter 确认) ")
                    .style(Style::default().bg(Color::Rgb(32, 32, 38))),
            );
        f.render_widget(list, popup_area);
    }

    // ── 文件路径 / 添加文件输入 ──
    let fp_focused = state.focus == FocusField::FilePath;
    let fp_style = if fp_focused {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default().fg(Color::White)
    };
    let cursor = if fp_focused { "▌" } else { "" };
    let fp_hint = if is_merge {
        format!(" 添加文件 (Enter 添加, 已添加 {} 个) ", state.merge_files.len())
    } else {
        " 文件路径 (Enter/↓ 下一步) ".to_string()
    };
    let fp_text = if state.file_path.is_empty() && !fp_focused {
        Span::styled(
            if is_merge { "输入文件路径后按 Enter 添加到列表..." } else { "请输入文件路径..." },
            Style::default().fg(Color::DarkGray),
        )
    } else {
        Span::styled(format!("{}{}", state.file_path, cursor), fp_style)
    };
    let fp_block = Block::default()
        .borders(Borders::ALL)
        .border_style(if fp_focused { Style::default().fg(Color::Yellow) } else { Style::default().fg(Color::DarkGray) })
        .title(fp_hint);
    let fp_widget = Paragraph::new(Line::from(vec![fp_text])).block(fp_block);
    f.render_widget(fp_widget, chunks[2]);

    if is_merge {
        // ── 文件列表 (合并模式) ──
        let fl_focused = state.focus == FocusField::FileList;
        let fl_items: Vec<ListItem> = if state.merge_files.is_empty() {
            vec![ListItem::new("  (空 — 在上方输入文件路径后按 Enter 添加)")
                .style(Style::default().fg(Color::DarkGray))]
        } else {
            state.merge_files.iter().enumerate().map(|(i, path)| {
                let style = if fl_focused && i == state.selected_file {
                    Style::default().fg(Color::Black).bg(Color::Yellow)
                } else {
                    Style::default().fg(Color::White)
                };
                let prefix = if fl_focused && i == state.selected_file { "▶ " } else { "  " };
                ListItem::new(format!("{}{:>3}. {}", prefix, i + 1, path)).style(style)
            }).collect()
        };
        let fl_block = Block::default()
            .borders(Borders::ALL)
            .border_style(if fl_focused { Style::default().fg(Color::Yellow) } else { Style::default().fg(Color::DarkGray) })
            .title(" 合并文件列表 (↑↓ 选择, Del 删除, ↓ 下一步) ");
        let fl_widget = List::new(fl_items).block(fl_block);
        f.render_widget(fl_widget, chunks[3]);
    }

    // ── 分组数 N (仅非合并模式显示) ──
    if !is_merge {
        let ng_focused = state.focus == FocusField::NumGroups;
        let ng_style = if ng_focused {
            Style::default().fg(Color::Green)
        } else {
            Style::default().fg(Color::White)
        };
        let ng_cursor = if ng_focused { "▌" } else { "" };
        let ng_block = Block::default()
            .borders(Borders::ALL)
            .border_style(if ng_focused { Style::default().fg(Color::Green) } else { Style::default().fg(Color::DarkGray) })
            .title(" 分组数 N (仅数字) ");
        let ng_widget = Paragraph::new(Line::from(vec![
            Span::styled(format!("{}{}", state.num_groups_str, ng_cursor), ng_style)
        ])).block(ng_block);
        f.render_widget(ng_widget, chunks[3]);
    }

    // ── 数据类型 ──
    let dt_focused = state.focus == FocusField::DataType;
    let dt_block = Block::default()
        .borders(Borders::ALL)
        .border_style(if dt_focused || state.dtype_open {
            Style::default().fg(Color::Magenta)
        } else {
            Style::default().fg(Color::DarkGray)
        })
        .title(" 数据块类型 (← → 切换 / Enter 选择 / ↓ 下一步) ");
    let dt_label = Span::styled(
        state.current_dtype().label(),
        if dt_focused { Style::default().fg(Color::Magenta) } else { Style::default().fg(Color::White) },
    );
    let dt_widget = Paragraph::new(Line::from(vec![dt_label])).block(dt_block);
    f.render_widget(dt_widget, chunks[4]);

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
    let action_text = if action_focused {
        format!(" ▶ 执行 {} (Enter) ", state.current_mode().short_label())
    } else {
        format!("   执行 {}   ", state.current_mode().short_label())
    };
    let action_widget = Paragraph::new(Line::from(vec![
        Span::styled(action_text, action_style)
    ])).block(action_block);
    f.render_widget(action_widget, chunks[5]);

    // ── 消息区 ──
    let msg_style = if state.message_is_error {
        Style::default().fg(Color::Red)
    } else {
        Style::default().fg(Color::Green)
    };
    let msg_text = state.message.as_deref().unwrap_or("就绪 — 选择模式、填写参数后按 Enter 执行");
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
    f.render_widget(msg_widget, chunks[6]);

    // ── 帮助栏 ──
    let help = Paragraph::new(Line::from(vec![
        Span::styled(" Tab/↓ ", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
        Span::styled("下一字段  ", Style::default().fg(Color::DarkGray)),
        Span::styled(" ↑ ", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
        Span::styled("上一字段  ", Style::default().fg(Color::DarkGray)),
        Span::styled(" Enter ", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
        Span::styled("确认/执行  ", Style::default().fg(Color::DarkGray)),
        Span::styled(" ← → ", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
        Span::styled("切换选项  ", Style::default().fg(Color::DarkGray)),
        Span::styled(" q/Esc ", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
        Span::styled("退出", Style::default().fg(Color::DarkGray)),
    ]))
    .block(Block::default().borders(Borders::TOP).border_style(Style::default().fg(Color::DarkGray)));
    f.render_widget(help, chunks[7]);
}

/// 居中矩形辅助
fn centered_rect(width: u16, height: u16, area: Rect) -> Rect {
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    Rect::new(x, y, width.min(area.width), height.min(area.height))
}

// ── 主循环 ────────────────────────────────────────────────────────────────────

fn main() -> Result<(), Box<dyn std::error::Error>> {
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

    disable_raw_mode()?;
    stdout().execute(LeaveAlternateScreen)?;
    Ok(())
}

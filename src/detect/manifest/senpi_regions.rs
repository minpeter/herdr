//! Senpi's current dock, distinct from the legacy generic border heuristics.
//!
//! The snapshot owns every byte. Parsing uses only borrowed slices and line
//! offsets; the manifest evaluator caches this view once, not once per rule.

pub(super) const ENGINE_VERSION: u32 = 4;

#[derive(Debug, Clone, Copy)]
pub(super) enum Region {
    Dialog,
    Status,
    BtwPanel,
    Footer,
}

impl Region {
    pub(super) fn parse(name: &str) -> Option<Self> {
        match name.trim() {
            "senpi_current_dialog" => Some(Self::Dialog),
            "senpi_current_status" => Some(Self::Status),
            "senpi_current_btw_panel" => Some(Self::BtwPanel),
            "senpi_current_footer" => Some(Self::Footer),
            _ => None,
        }
    }
}

#[derive(Default)]
pub(super) struct Regions<'a> {
    dialog: &'a str,
    status: &'a str,
    btw_panel: &'a str,
    footer: &'a str,
}

impl<'a> Regions<'a> {
    pub(super) fn parse(screen: &'a str) -> Self {
        let mut borders =
            lines_rev(screen).filter_map(|line| border(line.text).map(|shape| (line, shape)));
        let Some((bottom, bottom_shape)) = borders.next() else {
            // Legacy captures can end at an unbordered prompt. Retain that
            // bounded prefix, never the entire snapshot or a pasted prompt body.
            let Some(prompt) = lines_rev(screen).find(|line| prompt_line(line.text)) else {
                return Self::default();
            };
            return Self::dock(&screen[..prompt.start]);
        };
        let Some((top, top_shape)) = borders.next() else {
            return Self::default();
        };
        if top_shape.width != bottom_shape.width || bottom_shape.scrolled_up {
            return Self::default();
        }
        let body = &screen[top.end..bottom.start];
        let first = body
            .lines()
            .find(|line| !line.trim().is_empty())
            .unwrap_or("");
        let editor = prompt_line(first)
            || (top_shape.scrolled_up
                && body
                    .lines()
                    .all(|line| line.starts_with("  ") || line.trim().is_empty()));
        let dialog = first.starts_with(" Permission required: ") || first.trim_end() == " Feedback";
        if !editor && !dialog {
            // Do not search behind an unknown current selector for an old editor.
            return Self::default();
        }
        let mut result = Self::dock(&screen[..top.start]);
        if dialog {
            result.dialog = body;
        }
        result.footer = screen[bottom.end..]
            .lines()
            .rev()
            .find(|line| !line.trim().is_empty())
            .filter(|line| !line.starts_with([' ', '\t']))
            .unwrap_or("");
        result
    }

    pub(super) const fn get(&self, region: Region) -> &'a str {
        match region {
            Region::Dialog => self.dialog,
            Region::Status => self.status,
            Region::BtwPanel => self.btw_panel,
            Region::Footer => self.footer,
        }
    }

    fn dock(prefix: &'a str) -> Self {
        let mut result = Self::default();
        let mut block = DockBlock::Unknown;
        let mut status_start = 0;
        let mut rows = lines(prefix).peekable();
        while let Some(line) = rows.next() {
            if line.text.trim().is_empty() {
                continue;
            }
            if let Some(shape) = border(line.text) {
                if !block.valid_todo() {
                    result.status = "";
                }
                if rows
                    .peek()
                    .is_some_and(|next| next.text.starts_with(" btw: "))
                {
                    // Text pads the question/answer, so its user-controlled
                    // horizontal lines cannot close this column-zero panel.
                    let closing = rows.find(|next| border(next.text).is_some());
                    if let Some(closing) = closing.filter(|closing| {
                        border(closing.text)
                            .is_some_and(|closing_shape| closing_shape.width == shape.width)
                    }) {
                        result.btw_panel = &prefix[line.start..closing.end];
                        block = DockBlock::Auxiliary;
                        continue;
                    }
                }
                result.status = "";
                result.btw_panel = "";
                block = DockBlock::Unknown;
                continue;
            }
            if matches!(block, DockBlock::Todo { .. }) && line.text.starts_with(' ') {
                if todo_task(line.text) {
                    block = DockBlock::Todo { has_task: true };
                }
                continue;
            }
            if line.text.trim_end() == " Todo" {
                block = DockBlock::Todo { has_task: false };
                continue;
            }
            let trimmed = line.text.trim_start();
            if trimmed.starts_with("Tip:")
                || trimmed.starts_with('↳')
                || trimmed.starts_with("Running PreToolUse hook")
                || trimmed.starts_with("Running PostToolUse hook")
            {
                block = DockBlock::Auxiliary;
                continue;
            }
            if spinner_line(trimmed) {
                status_start = line.start;
                result.status = &prefix[status_start..line.end];
                block = DockBlock::Primary {
                    compaction: trimmed.contains(" to cancel)") && trimmed.contains("ompacting"),
                    stale_rows: 0,
                };
                continue;
            }
            match &mut block {
                DockBlock::Primary {
                    compaction,
                    stale_rows,
                } if line.text.starts_with([' ', '\t']) || (*compaction && *stale_rows < 3) => {
                    // Older compaction captures include a few unindented
                    // overwritten continuation rows. They cannot cross a border.
                    *stale_rows += 1;
                    result.status = &prefix[status_start..line.end];
                }
                DockBlock::Auxiliary if line.text.starts_with([' ', '\t']) => {}
                DockBlock::Unknown
                | DockBlock::Primary { .. }
                | DockBlock::Auxiliary
                | DockBlock::Todo { .. } => {
                    result.status = "";
                    result.btw_panel = "";
                    block = DockBlock::Unknown;
                }
            }
        }
        if !block.valid_todo() {
            result.status = "";
            result.btw_panel = "";
        }
        result
    }
}

#[derive(Default)]
enum DockBlock {
    #[default]
    Unknown,
    Primary {
        compaction: bool,
        stale_rows: usize,
    },
    Auxiliary,
    Todo {
        has_task: bool,
    },
}

impl DockBlock {
    const fn valid_todo(&self) -> bool {
        !matches!(self, Self::Todo { has_task: false })
    }
}

fn todo_task(line: &str) -> bool {
    [" [ ] ", " [•] ", " [✓] ", " [×] "]
        .iter()
        .any(|prefix| line.starts_with(prefix))
}

fn spinner_line(line: &str) -> bool {
    let mut chars = line.chars();
    matches!(
        chars.next(),
        Some(
            '•' | '◦'
                | '●'
                | '⠋'
                | '⠙'
                | '⠹'
                | '⠸'
                | '⠼'
                | '⠴'
                | '⠦'
                | '⠧'
                | '⠇'
                | '⠏'
        )
    ) && chars.next() == Some(' ')
}

fn prompt_line(line: &str) -> bool {
    line.trim_end() == "❯" || line.starts_with("❯ ")
}

struct Border {
    width: usize,
    scrolled_up: bool,
}

fn border(line: &str) -> Option<Border> {
    // Keep indentation: trim_start() is exactly what lets editor content
    // impersonate chrome in the legacy generic region.
    let line = line.trim_end();
    if !line.starts_with("───") {
        return None;
    }
    let scrolled_up = line.starts_with("─── ↑ ");
    if let Some(scroll) = line
        .strip_prefix("─── ↑ ")
        .or_else(|| line.strip_prefix("─── ↓ "))
    {
        let (count, tail) = scroll.split_once(" more ")?;
        if count.is_empty()
            || count.starts_with('0')
            || !count.bytes().all(|byte| byte.is_ascii_digit())
            || !tail.chars().all(|ch| ch == '─')
        {
            return None;
        }
    } else if !line.chars().all(|ch| ch == '─') {
        return None;
    }
    Some(Border {
        width: line.chars().count(),
        scrolled_up,
    })
}

struct Line<'a> {
    start: usize,
    end: usize,
    text: &'a str,
}

fn lines(text: &str) -> impl Iterator<Item = Line<'_>> {
    let mut offset = 0;
    text.split_inclusive('\n').map(move |chunk| {
        let start = offset;
        offset += chunk.len();
        Line {
            start,
            end: offset,
            text: chunk.trim_end_matches(['\r', '\n']),
        }
    })
}

fn lines_rev(text: &str) -> impl Iterator<Item = Line<'_>> {
    let mut offset = text.len();
    text.split_inclusive('\n').rev().map(move |chunk| {
        let end = offset;
        offset -= chunk.len();
        Line {
            start: offset,
            end,
            text: chunk.trim_end_matches(['\r', '\n']),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indented_separators_remain_inside_the_editor() {
        let screen =
            " ⠋ Working\n──────────\n❯ text\n  ──────\n  ⠋ Running eval\n──────────\nfooter\n";
        let parsed = Regions::parse(screen);
        assert_eq!(parsed.get(Region::Status), " ⠋ Working\n");
        assert_eq!(parsed.get(Region::Footer), "footer");
        assert_eq!(parsed.get(Region::Dialog), "");
    }

    #[test]
    fn scrolled_editor_cannot_be_a_permission_dialog() {
        let screen = "─── ↑ 1 more ───────\n  Permission required: bash\n  → Allow once\n────────────────────\nfooter\n";
        let parsed = Regions::parse(screen);
        assert_eq!(parsed.get(Region::Dialog), "");
        assert_eq!(parsed.get(Region::Footer), "footer");
    }

    #[test]
    fn unknown_current_dialog_does_not_expose_the_previous_editor() {
        let screen = " ⠋ Working\n──────────\n❯\n──────────\n──────────\n Unrelated dialog\n──────────\nfooter\n";
        let parsed = Regions::parse(screen);
        assert_eq!(parsed.get(Region::Status), "");
        assert_eq!(parsed.get(Region::Dialog), "");
    }

    #[test]
    fn selected_regions_are_borrowed_for_varied_unicode_and_line_endings() {
        for width in [20, 40, 80, 110] {
            for text in ["plain", "한글", "😺", "e\u{301}", "─"] {
                for newline in ["\n", "\r\n"] {
                    let border = "─".repeat(width);
                    let screen = format!(" ⠋ Working{newline}{border}{newline}❯ {text}{newline}{border}{newline}{text}{newline}");
                    let parsed = Regions::parse(&screen);
                    assert_eq!(parsed.get(Region::Status).trim_end(), " ⠋ Working");
                    assert_eq!(parsed.get(Region::Footer), text);
                    for region in [
                        Region::Status,
                        Region::Dialog,
                        Region::BtwPanel,
                        Region::Footer,
                    ] {
                        let selected = parsed.get(region);
                        if !selected.is_empty() {
                            let offset = selected.as_ptr().addr() - screen.as_ptr().addr();
                            assert_eq!(&screen[offset..offset + selected.len()], selected);
                        }
                    }
                }
            }
        }
    }
}

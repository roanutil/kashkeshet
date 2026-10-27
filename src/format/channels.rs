//! Output for channels and their members.

use super::agents::with_description;
use super::fit::{Row, cut_body, one_line};
use super::{Format, Formatter};
use crate::config::NOTE_PREVIEW_CHARS;
use crate::rules::{ChannelInfo, ChannelNote, Members, Page};
use serde_json::{Value, json};

impl Formatter {
    /// A lossless slice of the note, sized after rendering (including JSON escapes).
    pub fn note(&self, f: Format, note: &ChannelNote, offset: usize) -> String {
        let boundaries: Vec<usize> = note
            .text
            .char_indices()
            .map(|(i, _)| i)
            .chain(std::iter::once(note.text.len()))
            .collect();
        let offset = offset.min(boundaries.len() - 1);
        let remaining = boundaries.len() - 1 - offset;
        let render = |count: usize| {
            let text = &note.text[boundaries[offset]..boundaries[offset + count]];
            let next = (count < remaining).then_some(offset + count);
            match f {
                Format::Json => json!({"channel_id": note.channel_id, "text": text,
                    "edited_by": note.edited_by, "edited_at": note.edited_at,
                    "offset": offset, "next_offset": next})
                .to_string(),
                Format::Text => {
                    let mut out = format!(
                        "note [{}]\nlast edited by {} at {}\noffset: {offset}\n{text}",
                        note.channel_id,
                        note.edited_by.as_deref().unwrap_or("(never)"),
                        note.edited_at.as_deref().unwrap_or("(never)")
                    );
                    if let Some(next) = next {
                        out.push_str(&format!(
                            "\nnext_offset: {next} (get_note {} offset={next})",
                            note.channel_id
                        ));
                    }
                    out
                }
            }
        };
        let full = render(remaining);
        if full.len() <= self.max_bytes {
            return full;
        }
        let (mut low, mut high) = (0, remaining);
        while low < high {
            let mid = low + (high - low).div_ceil(2);
            if render(mid).len() <= self.max_bytes {
                low = mid;
            } else {
                high = mid - 1;
            }
        }
        render(low)
    }

    pub fn channels(&self, f: Format, page: &Page<ChannelInfo>, purpose_chars: usize) -> String {
        let rows: Vec<Row> = page
            .items
            .iter()
            .map(|c| {
                let mut note_chars = NOTE_PREVIEW_CHARS;
                loop {
                    let row = Row {
                        text: channel_line(c, purpose_chars, note_chars),
                        json: channel_json(c, Some(purpose_chars), note_chars),
                        resume: format!("cursor={}", crate::rules::channel_cursor(c)),
                    };
                    let bytes = match f {
                        Format::Text => row.text.len() + "\nmore: ".len() + row.resume.len(),
                        Format::Json => json!({"channels": [&row.json], "more": row.resume})
                            .to_string()
                            .len(),
                    };
                    if bytes <= self.max_bytes || note_chars == 0 {
                        break row;
                    }
                    note_chars -= 1;
                }
            })
            .collect();
        let tail = page.next_cursor.as_ref().map(|c| format!("cursor={c}"));
        self.list(f, None, "channels", &rows, tail)
    }

    /// One channel's metadata. Pinned notes are read separately with `get_note`.
    pub fn channel(&self, f: Format, c: &ChannelInfo) -> String {
        let out = match f {
            Format::Text => {
                let mut lines = vec![format!(
                    "#{} [{}] {}, created by {}",
                    c.name,
                    c.id,
                    if c.open { "open" } else { "closed" },
                    c.created_by
                )];
                if let Some(p) = &c.purpose {
                    lines.push(format!("purpose: {}", one_line(p)));
                }
                lines.push(format!(
                    "members ({}): {}{}",
                    c.member_count,
                    c.members_preview.join(", "),
                    if (c.members_preview.len() as i64) < c.member_count {
                        ", …"
                    } else {
                        ""
                    }
                ));
                lines.push(
                    if c.is_member {
                        "you are a member"
                    } else {
                        "you are not a member"
                    }
                    .into(),
                );
                lines.join("\n")
            }
            Format::Json => channel_json(c, None, NOTE_PREVIEW_CHARS).to_string(),
        };
        self.clip(out)
    }

    pub fn members(&self, f: Format, m: &Members) -> String {
        let header = (
            format!("{}: {} members", m.channel_id, m.total),
            json!({ "channel_id": m.channel_id, "total": m.total }),
        );
        let rows = self.described_rows(
            f,
            Some(&header),
            "members",
            &m.page.items,
            |a, description| Row {
                text: with_description(format!("{}; {}; last call {}", a.name, super::agents::listening_text(a), a.last_seen.as_deref().unwrap_or("never")), description),
                json: json!({ "name": a.name, "description": description, "last_seen": a.last_seen, "listening": a.listening, "waiting_channels": a.waiting_channels }),
                resume: format!("cursor={}", a.name),
            },
        );
        let tail = m.page.next_cursor.as_ref().map(|c| format!("cursor={c}"));
        self.list(f, Some(header), "members", &rows, tail)
    }
}

fn channel_line(c: &ChannelInfo, purpose_chars: usize, note_chars: usize) -> String {
    let mut parts = vec![
        format!("[{}] #{}", c.id, c.name),
        (if c.open { "open" } else { "closed" }).to_string(),
    ];
    let more = if (c.members_preview.len() as i64) < c.member_count {
        ", …"
    } else {
        ""
    };
    parts.push(format!(
        "{} members: {}{}",
        c.member_count,
        c.members_preview.join(", "),
        more
    ));
    if c.is_member {
        parts.push("you're a member".into());
    }
    if let Some(n) = c.new_messages {
        parts.push(format!("{n} new"));
    }
    let mut line = parts.join(" · ");
    if let Some(p) = &c.purpose {
        line.push_str(" — ");
        line.push_str(&cut_body(&one_line(p), Some(purpose_chars)).0);
    }
    if c.note.edited_by.is_some() {
        line.push_str(&format!(
            " · note: {} ({} at {}; get_note {})",
            cut_body(&one_line(&c.note.text), Some(note_chars)).0,
            c.note.edited_by.as_deref().unwrap_or(""),
            c.note.edited_at.as_deref().unwrap_or(""),
            c.id
        ));
    }
    line
}

fn channel_json(c: &ChannelInfo, purpose_chars: Option<usize>, note_chars: usize) -> Value {
    let purpose = match (c.purpose.as_deref(), purpose_chars) {
        (Some(p), Some(max)) => Some(cut_body(p, Some(max)).0),
        (p, _) => p.map(str::to_string),
    };
    let mut value = json!({
        "id": c.id, "name": c.name, "purpose": purpose, "open": c.open,
        "created_by": c.created_by, "member_count": c.member_count,
        "members_preview": c.members_preview, "is_member": c.is_member,
        "new_messages": c.new_messages,
    });
    if purpose_chars.is_some() {
        value["note"] = json!(cut_body(&c.note.text, Some(note_chars)).0);
        value["note_cut"] = json!(c.note.text.chars().count() > note_chars);
        value["note_edited_by"] = json!(c.note.edited_by);
        value["note_edited_at"] = json!(c.note.edited_at);
    }
    value
}

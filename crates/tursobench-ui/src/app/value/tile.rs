//! The record tile's second level: one field filling the tile body, read
//! through a lens, under a header that is the field's own row.
use super::{
    Decoded, Lens, Summary, Target, hex, inspect,
    job::Work,
    lens::{self, Shown, Size},
    view::{CHUNK, CLIPBOARD, LOAD, Source, View, Want},
};
use crate::app::{
    App, files,
    icons::{self, Icon},
    presentation,
};
use eframe::egui;
use std::sync::Arc;
use tursobench_blob::{Kind, Order, encode::Format, tree::Part};
use tursobench_core::{EventKind, Value};
use tursobench_db::Operation;

/// What the header asked for.
enum Header {
    Back,
    Lens(Lens),
    Find,
    Copy(Format),
    CopyImage,
    Save,
}

impl App {
    /// The value field `col` of `row` shows: its draft, else the stored value.
    fn shown_value(&self, row: usize, col: usize) -> Option<&Value> {
        let document = &self.database.document;
        let values = document.result.rows.get(row)?;
        document
            .drafts
            .as_ref()
            .and_then(|d| d.displayed(values, col))
            .or_else(|| values.get(col))
    }
    fn value_view(&mut self) -> Option<&mut View> {
        self.database.document.tools.values.view.as_mut()
    }
    pub(in crate::app) fn value_open(&self) -> bool {
        self.database.document.tools.values.view.is_some()
    }

    /// Open field `col` of `row` in the value view, filling the record tile.
    pub(in crate::app) fn open_value(&mut self, row: usize, col: usize) {
        let document = &self.database.document;
        let from = (document.result.current, document.draft_revision);
        let Some(view) = self
            .shown_value(row, col)
            .and_then(|value| View::new(row, col, value, from))
        else {
            return;
        };
        self.open_record_window(row, col);
        self.database.document.ui.inspector_detached = false;
        self.database.document.tools.values.view = Some(view);
    }
    pub(in crate::app) fn close_value(&mut self) {
        self.database.document.tools.values.view = None;
        self.database.document.tools.values.idle();
    }

    /// Follow the record stepper: the same field of the record now selected.
    /// Returns whether a value is still open.
    fn sync_value_view(&mut self) -> bool {
        let document = &self.database.document;
        let from = (document.result.current, document.draft_revision);
        let row = document.inspect.map(|(row, _)| row);
        let Some(view) = &document.tools.values.view else {
            return false;
        };
        if row == Some(view.row) && from == view.from {
            return true;
        }
        let (col, order, characters, lens) = (view.col, view.order, view.characters, view.lens);
        let next = row
            .and_then(|row| Some((row, self.shown_value(row, col)?)))
            .and_then(|(row, value)| View::new(row, col, value, from));
        let values = &mut self.database.document.tools.values;
        values.ticket = None;
        values.view = next.map(|mut view| {
            (view.order, view.characters, view.lens) = (order, characters, lens);
            view
        });
        values.view.is_some()
    }

    /// The kind and lens of the open value.
    fn value_lens(&self, ctx: &egui::Context) -> Option<(Kind, Lens)> {
        let view = self.database.document.tools.values.view.as_ref()?;
        let value = self.shown_value(view.row, view.col)?;
        let kind = match value {
            Value::Text(_) => Kind::Text(tursobench_blob::text::Encoding::Utf8),
            other => Summary::of(ctx, other)?.kind,
        };
        let chosen = self.database.document.tools.columns.lens(view.col);
        Some((kind, Lens::resolve(kind, chosen)))
    }

    /// The value view: header, body through the lens, and footer.
    pub(in crate::app) fn value_view_ui(&mut self, ui: &mut egui::Ui) {
        if !self.sync_value_view() {
            return;
        }
        self.poll_value_save();
        let Some((kind, lens)) = self.value_lens(ui.ctx()) else {
            self.close_value();
            return;
        };
        if let Some(header) = self.value_header(ui, kind, lens) {
            self.header_action(ui.ctx(), header);
            if !self.value_open() {
                return;
            }
        }
        egui::Panel::bottom("value-footer")
            .resizable(false)
            .exact_size(24.0)
            .frame(egui::Frame::new().inner_margin(egui::Margin::symmetric(4, 0)))
            .show(ui, |ui| self.value_footer(ui, lens));
        match lens {
            Lens::Hex => self.hex_body(ui),
            Lens::Text => self.text_body(ui, kind),
            Lens::Tree => self.tree_body(ui),
            Lens::Image => self.image_body(ui),
        }
        if self.record_tile_active() {
            self.value_keys(ui.ctx());
        }
        self.drive_value();
    }

    /// The field's own row as the header: back, name, summary and tools.
    fn value_header(&mut self, ui: &mut egui::Ui, kind: Kind, lens: Lens) -> Option<Header> {
        let view = self.database.document.tools.values.view.as_ref()?;
        let name = self
            .database
            .document
            .result
            .columns
            .get(view.col)?
            .name
            .clone();
        let summary = self
            .shown_value(view.row, view.col)
            .and_then(|value| Summary::of(ui.ctx(), value));
        let (rect, _) =
            ui.allocate_exact_size(egui::vec2(ui.available_width(), 24.0), egui::Sense::hover());
        let tokens = crate::appearance::Tokens::of(ui.ctx());
        ui.painter().hline(
            rect.x_range(),
            rect.bottom(),
            egui::Stroke::new(1.0, tokens.line_soft),
        );
        let mut action = None;
        let back = egui::Rect::from_min_size(rect.min, egui::vec2(20.0, 24.0));
        if line_button(ui, back, Icon::Back, "Back to fields", "Esc", None) {
            action = Some(Header::Back);
        }
        let width = presentation::text_width(ui, name.as_str(), 12.5) + 8.0;
        let label =
            egui::Rect::from_min_size(rect.min + egui::vec2(22.0, 0.0), egui::vec2(width, 24.0));
        presentation::text_at(ui, label, &name, ui.visuals().text_color(), 12.5);
        // Tools from the right: save, copy as, find, then the lenses.
        let mut right = rect.right();
        let mut slot = |gap: f32| {
            right -= 24.0 + gap;
            egui::Rect::from_min_size(egui::pos2(right, rect.top()), egui::vec2(24.0, 24.0))
        };
        if line_button(ui, slot(0.0), Icon::Export, "Save to file…", "", None) {
            action = Some(Header::Save);
        }
        action = self
            .copy_menu(ui, slot(0.0), lens == Lens::Image)
            .or(action);
        if lens == Lens::Hex && line_button(ui, slot(0.0), Icon::Search, "Find", "Ctrl F", None) {
            action = Some(Header::Find);
        }
        let offered = Lens::offered(kind);
        for (i, &each) in offered
            .iter()
            .rev()
            .enumerate()
            .filter(|_| offered.len() > 1)
        {
            let gap = if i == 0 { 6.0 } else { 0.0 };
            if line_button(
                ui,
                slot(gap),
                each.icon(),
                each.name(),
                "",
                Some(each == lens),
            ) {
                action = Some(Header::Lens(each));
            }
        }
        // The left edge of the leftmost tool.
        let right = slot(0.0).right();
        if let Some(summary) = summary {
            let rest = egui::Rect::from_min_max(
                egui::pos2(label.right(), rect.top()),
                egui::pos2(right - 4.0, rect.bottom()),
            );
            summary.paint(ui, rest, ui.visuals().text_color(), false, None);
        }
        action
    }

    /// The copy-as icon and its menu of formats, with the image itself for
    /// the image lens; returns what was chosen.
    fn copy_menu(&mut self, ui: &mut egui::Ui, rect: egui::Rect, image: bool) -> Option<Header> {
        let mut child = ui.new_child(egui::UiBuilder::new().id_salt("value-copy").max_rect(rect));
        let response = icons::line_control(&mut child, Icon::Copy, "Copy as…", "", None);
        let mut chosen = None;
        egui::Popup::menu(&response).show(|ui| {
            if image && ui.button("Image").clicked() {
                chosen = Some(Header::CopyImage);
            }
            for format in Format::ALL {
                if ui.button(format.name()).clicked() {
                    chosen = Some(Header::Copy(format));
                }
            }
        });
        chosen
    }

    fn header_action(&mut self, ctx: &egui::Context, header: Header) {
        match header {
            Header::Back => self.close_value(),
            Header::Lens(lens) => {
                if let Some(col) = self.value_view().map(|v| v.col) {
                    self.database
                        .document
                        .tools
                        .columns
                        .lenses
                        .insert(col, lens);
                }
            }
            Header::Find => {
                if let Some(view) = self.value_view() {
                    view.find.get_or_insert_with(String::new);
                    view.goto = None;
                }
            }
            Header::Copy(format) => self.copy_value(ctx, format, false),
            Header::CopyImage => self.copy_image(ctx),
            Header::Save => self.save_value(ctx),
        }
    }

    /// The paged hex lens, with click and drag selection, the wheel and a scroll bar.
    fn hex_body(&mut self, ui: &mut egui::Ui) {
        let color = ui.visuals().text_color();
        let rect = ui.available_rect_before_wrap();
        let Some(view) = self.value_view() else {
            return;
        };
        let body = egui::Rect::from_min_max(
            rect.min + egui::vec2(4.0, 2.0),
            rect.max - egui::vec2(12.0, 2.0),
        );
        let grid = hex::Grid::new(ui, body, view.source.len());
        (view.per, view.lines) = (grid.per, grid.lines());
        let response = ui.interact(
            body,
            ui.scope_id().with("value-hex"),
            egui::Sense::click_and_drag(),
        );
        if let Some(pos) = response.interact_pointer_pos()
            && (response.clicked() || response.dragged() || response.drag_started())
            && let Some((at, characters)) = grid.hit(pos, view.top)
        {
            let extend =
                response.dragged() && !response.drag_started() || ui.input(|i| i.modifiers.shift);
            view.characters = characters;
            view.move_to(at, extend);
        }
        if response.hovered() {
            let delta = ui.input(|i| i.smooth_scroll_delta.y);
            if delta != 0.0 {
                view.scroll_to_line(view.top as i64 - (delta / hex::LINE).round() as i64);
            }
        }
        let total = view.source.len().div_ceil(grid.per as u64).max(1);
        let track = egui::Rect::from_min_max(
            egui::pos2(rect.right() - 8.0, body.top()),
            egui::pos2(rect.right() - 4.0, body.bottom()),
        );
        if let Some(line) = hex::scrollbar(ui, track, view.top, grid.lines(), total) {
            view.scroll_to_line(line);
        }
        hex::paint(ui, &grid, view, color);
        let label = format!("Byte {} of {}", view.cursor, view.source.len());
        response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Grid, true, &label));
        ui.allocate_rect(rect, egui::Sense::hover());
    }

    /// The text lens: the value's text, wrapped, from memory or loaded whole.
    fn text_body(&mut self, ui: &mut egui::Ui, kind: Kind) {
        let Some(view) = self.value_view() else {
            return;
        };
        let Some(bytes) = view.whole() else {
            return loading(ui, view);
        };
        let shown = &bytes[..bytes.len().min(1024 * 1024)];
        let text = match kind {
            Kind::Text(encoding) if !view.text => tursobench_blob::text::decode(shown, encoding),
            _ => String::from_utf8_lossy(shown).into_owned(),
        };
        egui::ScrollArea::vertical()
            .id_salt("value-text")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.add(egui::Label::new(egui::RichText::new(text).size(12.5)).wrap())
            });
    }

    /// The tree lens: the whole value decoded off the UI thread, printed as JSON.
    fn tree_body(&mut self, ui: &mut egui::Ui) {
        let document = &mut self.database.document;
        let values = &mut document.tools.values;
        let Some(view) = values.view.as_mut() else {
            return;
        };
        let (row, col) = (view.row, view.col);
        let Some(bytes) = view.whole() else {
            return loading(ui, view);
        };
        let target = Target {
            result: document.result.current,
            drafts: document.draft_revision,
            row,
            col,
            whole: true,
        };
        let decoded = values.tree(ui.ctx(), target, || Work::Jsonb(bytes));
        let tree = match decoded {
            Decoded::Tree(tree) => tree,
            Decoded::Pending => return icons::empty_state(ui, Icon::Tree, "Reading"),
            Decoded::Failed(reason) => {
                return icons::empty_state(
                    ui,
                    Icon::Tree,
                    &format!("Not readable as a tree: {reason}"),
                );
            }
        };
        let lines = tree.pretty(usize::MAX);
        let key = ui.visuals().hyperlink_color;
        let value = crate::appearance::Tokens::of(ui.ctx()).relationship;
        let plain = ui.visuals().text_color();
        egui::ScrollArea::vertical()
            .id_salt("value-tree")
            .auto_shrink([false, false])
            .show_rows(ui, hex::LINE, lines.len(), |ui, range| {
                ui.spacing_mut().item_spacing.y = 0.0;
                for line in &lines[range] {
                    let mut job = egui::text::LayoutJob::default();
                    let font = super::preview::font(ui);
                    job.append(
                        &"  ".repeat(line.depth),
                        0.0,
                        egui::TextFormat::simple(font.clone(), plain),
                    );
                    for (text, part) in &line.parts {
                        let color = match part {
                            Part::Key | Part::Literal => key,
                            Part::Text | Part::Number => value,
                            Part::Punctuation => plain,
                        };
                        job.append(text, 0.0, egui::TextFormat::simple(font.clone(), color));
                    }
                    let (rect, _) = ui.allocate_exact_size(
                        egui::vec2(ui.available_width(), hex::LINE),
                        egui::Sense::hover(),
                    );
                    job.wrap.max_width = rect.width() - 8.0;
                    job.wrap.max_rows = 1;
                    job.wrap.break_anywhere = true;
                    job.wrap.overflow_character = Some('…');
                    let galley = ui.painter().layout_job(job);
                    let at = rect.left_top() + egui::vec2(4.0, (hex::LINE - galley.size().y) / 2.0);
                    ui.painter().galley(at, galley, plain);
                }
            });
    }

    /// The value's image, decoded off the UI thread: fitted, or at the
    /// footer's zoom, and moved by dragging.
    fn image_body(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let document = &mut self.database.document;
        let (result, drafts) = (document.result.current, document.draft_revision);
        let values = &mut document.tools.values;
        let Some(view) = values.view.as_mut() else {
            return;
        };
        let (row, col) = (view.row, view.col);
        let Some(bytes) = view.whole() else {
            return loading(ui, view);
        };
        let target = Target {
            result,
            drafts,
            row,
            col,
            whole: true,
        };
        let image = || match tursobench_blob::sniff(&bytes, bytes.len() as u64) {
            Kind::Image { format, .. } => Some((bytes.clone(), format)),
            _ => None,
        };
        let (id, size, meta) = match values.images.get(&ctx, target, Size::Full, image) {
            Shown::Ready(id, size, meta) => (id, size, meta),
            Shown::Pending => return icons::empty_state(ui, Icon::Image, "Reading"),
            Shown::Failed(reason) => return icons::empty_state(ui, Icon::Image, &reason),
        };
        let Some(view) = values.view.as_mut() else {
            return;
        };
        let rect = ui.available_rect_before_wrap();
        let response = ui.interact(rect, ui.scope_id().with("value-image"), egui::Sense::drag());
        if response.dragged() {
            view.pan += response.drag_delta();
        }
        let scale = view
            .zoom
            .unwrap_or_else(|| lens::fit(rect.shrink(8.0), size));
        lens::paint_image(ui, rect, id, size, scale, view.pan);
        view.image = Some((meta, size, scale));
        let label = format!("Image, {} × {}, {}", meta.width, meta.height, meta.color);
        response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Image, true, &label));
        ui.allocate_rect(rect, egui::Sense::hover());
    }

    /// Decode the whole image again and put it on the clipboard.
    fn copy_image(&mut self, ctx: &egui::Context) {
        let document = &mut self.database.document;
        let (result, drafts) = (document.result.current, document.draft_revision);
        let values = &mut document.tools.values;
        let Some(view) = values.view.as_mut() else {
            return;
        };
        let (row, col) = (view.row, view.col);
        let Some(bytes) = view.whole() else {
            return;
        };
        view.message = "Copied the image".into();
        let target = Target {
            result,
            drafts,
            row,
            col,
            whole: true,
        };
        let image = || match tursobench_blob::sniff(&bytes, bytes.len() as u64) {
            Kind::Image { format, .. } => Some((bytes.clone(), format)),
            _ => None,
        };
        values.images.get(ctx, target, Size::Copy, image);
    }

    /// The footer: offset, selection and readings, or the find or go-to input.
    fn value_footer(&mut self, ui: &mut egui::Ui, lens: Lens) {
        let wide = ui.available_width() >= 480.0;
        let tokens = crate::appearance::Tokens::of(ui.ctx());
        let Some(view) = self.value_view() else {
            return;
        };
        ui.horizontal_centered(|ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
            if let Some(query) = view.find.as_mut() {
                let input = egui::TextEdit::singleline(query)
                    .hint_text("Find bytes (00 ff) or text")
                    .desired_width(ui.available_width() - 8.0);
                ui.add(input).request_focus();
                return;
            }
            if let Some(goto) = view.goto.as_mut() {
                let input = egui::TextEdit::singleline(goto)
                    .hint_text("Go to offset: 0x1a3d4 or 1024")
                    .desired_width(ui.available_width() - 8.0);
                ui.add(input).request_focus();
                return;
            }
            if !view.message.is_empty() {
                ui.label(
                    egui::RichText::new(&view.message)
                        .size(11.5)
                        .color(tokens.faint),
                );
                return;
            }
            if let (Lens::Image, Some(image)) = (lens, view.image) {
                return image_footer(ui, view, image);
            }
            if lens != Lens::Hex {
                let size = super::summary::byte_size(view.source.len());
                ui.label(egui::RichText::new(size).size(11.5).color(tokens.faint));
                return;
            }
            footer_readings(ui, view, wide);
        });
    }

    /// Keys while a value is open and nothing else takes them.
    fn value_keys(&mut self, ctx: &egui::Context) {
        if egui::Popup::is_any_open(ctx) || self.database.ui.palette_open {
            return;
        }
        let command = egui::Modifiers::COMMAND;
        let pressed = |key, modifiers| ctx.input_mut(|i| i.consume_key(modifiers, key));
        if pressed(egui::Key::Enter, egui::Modifiers::NONE) {
            return self.submit_value_input(ctx);
        }
        if ctx.egui_wants_keyboard_input() {
            return;
        }
        if pressed(egui::Key::F, command) {
            return self.header_action(ctx, Header::Find);
        }
        if pressed(egui::Key::G, command) {
            if let Some(view) = self.value_view() {
                view.goto = Some(String::new());
            }
            return;
        }
        if ctx.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::Copy))) {
            let characters = self.value_view().is_some_and(|v| v.characters);
            let format = if characters {
                Format::Text
            } else {
                Format::Hex
            };
            return self.copy_value(ctx, format, true);
        }
        let Some(view) = self.value_view() else {
            return;
        };
        let modifiers = ctx.input(|i| i.modifiers);
        let keys = [
            egui::Key::ArrowLeft,
            egui::Key::ArrowRight,
            egui::Key::ArrowUp,
            egui::Key::ArrowDown,
            egui::Key::PageUp,
            egui::Key::PageDown,
            egui::Key::Home,
            egui::Key::End,
            egui::Key::Tab,
        ];
        for key in keys {
            if ctx.input_mut(|i| i.consume_key(modifiers, key)) {
                view.key(key, modifiers.shift, modifiers.command);
            }
        }
    }

    /// Esc closes the find or go-to input first, then the value.
    pub(in crate::app) fn escape_value(&mut self, ctx: &egui::Context) {
        let Some(view) = self.value_view() else {
            return;
        };
        if view.find.take().is_some() || view.goto.take().is_some() {
            // The input is gone: give the keys back to the value.
            ctx.memory_mut(|m| m.stop_text_input());
            return;
        }
        if !view.message.is_empty() {
            view.message.clear();
            return;
        }
        self.close_value();
    }

    /// Enter searches or jumps from the footer's input.
    fn submit_value_input(&mut self, ctx: &egui::Context) {
        let Some(view) = self.value_view() else {
            return;
        };
        if let Some(goto) = view.goto.take() {
            ctx.memory_mut(|m| m.stop_text_input());
            match inspect::parse_offset(&goto) {
                Some(at) if at < view.source.len() => view.move_to(at, false),
                _ => view.message = format!("No offset {goto} in this value"),
            }
        } else if view.find.is_some() {
            self.find_value();
        }
    }

    /// Search for the typed bytes or text after the cursor: in memory when
    /// the whole value is held, otherwise in the backing.
    pub(in crate::app) fn find_value(&mut self) {
        let Some(view) = self.value_view() else {
            return;
        };
        let query = view.find.clone().unwrap_or_default();
        let needle = tursobench_blob::encode::parse_hex(&query)
            .unwrap_or_else(|| query.clone().into_bytes());
        if needle.is_empty() || needle.len() > 1024 {
            view.message = "Find 1–1,024 bytes".into();
            return;
        }
        // Searching again from a match finds the next one.
        let start = *view.selection().start();
        view.find_from = match view.found {
            Some(at) if at == start => at + 1,
            _ => view.cursor,
        };
        view.needle = needle;
        match view.whole() {
            Some(bytes) => {
                let start = (view.find_from as usize).min(bytes.len());
                let needle = view.needle.as_slice();
                let hit = bytes[start..]
                    .windows(needle.len())
                    .position(|w| w == needle);
                found(view, hit.map(|i| (start + i) as u64));
            }
            None => view.want = Some(Want::Find),
        }
    }

    /// Copy the selection, or with `selection` false and one byte selected the
    /// whole value, as `format`; a deferred value is loaded first.
    pub(in crate::app) fn copy_value(
        &mut self,
        ctx: &egui::Context,
        format: Format,
        selection: bool,
    ) {
        let Some(view) = self.value_view() else {
            return;
        };
        let (at, len) = if selection || view.selected() > 1 {
            (*view.selection().start(), view.selected())
        } else {
            (0, view.source.len())
        };
        self.copy_range(ctx, format, at, len);
    }
    /// Copy `len` bytes from `at` as `format`, loading a deferred value first.
    fn copy_range(&mut self, ctx: &egui::Context, format: Format, at: u64, len: u64) {
        let Some(view) = self.value_view() else {
            return;
        };
        if format.length(len as usize) > CLIPBOARD {
            view.message = "Over the 2 MiB clipboard budget · save to file instead".into();
            return;
        }
        if let Some(bytes) = view.bytes(at, len as usize) {
            ctx.copy_text(format.write(&bytes));
            view.message = format!("Copied {len} bytes as {}", format.name());
            return;
        }
        if view.source.handle().is_some() && view.source.len() <= LOAD {
            view.copy = Some((format, at, len));
            view.want = Some(Want::Load);
        } else {
            view.message = "The value is not held · save to file instead".into();
        }
    }

    /// Ask where to save the value; bytes in memory are written on the
    /// dialog's thread, a deferred value by the session.
    fn save_value(&mut self, ctx: &egui::Context) {
        let ctx = ctx.clone();
        let extension = self
            .value_lens(&ctx)
            .map_or("bin", |(kind, _)| kind.extension());
        let Some(view) = self.database.document.tools.values.view.as_ref() else {
            return;
        };
        let column = self
            .database
            .document
            .result
            .columns
            .get(view.col)
            .map_or("value", |c| c.name.as_str());
        let name = format!("{column}.{extension}");
        let bytes = match &view.source {
            Source::Bytes(bytes) => Some(bytes.clone()),
            Source::Deferred {
                handle: Some(_), ..
            } => None,
            Source::Deferred { handle: None, .. } => {
                return self.value_message("The value is too large to keep, so it cannot be saved");
            }
        };
        let task = files::start_task(ctx, "value-save-dialog", move || {
            files::save_value_dialog(&name, bytes)
        });
        match task {
            Ok(rx) => {
                if let Some(view) = self.value_view() {
                    view.save = Some(rx);
                }
            }
            Err(e) => self.value_message(&format!("Could not open the save dialog: {e}")),
        }
    }
    fn value_message(&mut self, message: &str) {
        if let Some(view) = self.value_view() {
            view.message = message.into();
        }
    }
    fn poll_value_save(&mut self) {
        let Some(view) = self.value_view() else {
            return;
        };
        let Some(rx) = &view.save else {
            return;
        };
        let result = match rx.try_recv() {
            Ok(result) => result.and_then(|saved| saved),
            Err(std::sync::mpsc::TryRecvError::Empty) => return,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                Err("The save dialog stopped".into())
            }
        };
        view.save = None;
        match result {
            Ok(Some(path)) => self.value_saved_to(path),
            Ok(None) => {}
            Err(e) => view.message = e,
        }
    }
    /// The dialog chose `path`: a value in memory is already written there;
    /// the session writes a deferred one.
    pub(in crate::app) fn value_saved_to(&mut self, path: std::path::PathBuf) {
        let Some(view) = self.value_view() else {
            return;
        };
        if view.source.handle().is_some() {
            view.save_path = Some(path);
            view.want = Some(Want::Save);
        } else {
            view.message = format!("Saved to {}", path.display());
        }
    }

    /// Send the request the view needs next, one at a time.
    pub(in crate::app) fn drive_value(&mut self) {
        let values = &mut self.database.document.tools.values;
        if values.ticket.is_some() {
            return;
        }
        let Some(view) = values.view.as_mut() else {
            return;
        };
        let Some(handle) = view.source.handle().filter(|_| !view.expired) else {
            return;
        };
        let Some(want) = view
            .want
            .take()
            .or_else(|| view.missing_chunk().map(Want::Chunk))
        else {
            return;
        };
        let operation = match want {
            Want::Chunk(index) => Operation::ReadValue {
                handle,
                offset: index * CHUNK,
                bytes: CHUNK as usize,
            },
            Want::Load => Operation::LoadValue { handle },
            Want::Find => Operation::FindValue {
                handle,
                needle: view.needle.clone(),
                from: view.find_from,
            },
            Want::Save => match view.save_path.take() {
                Some(path) => Operation::SaveValue { handle, path },
                None => return,
            },
        };
        view.pending = Some(want);
        let outcome = self.database.request(operation);
        let message = match outcome {
            Some(Ok(ticket)) => {
                self.database.document.tools.values.ticket = Some(ticket);
                return;
            }
            Some(Err(e)) => e.to_string(),
            None => "The source session is closed".into(),
        };
        if let Some(view) = self.value_view() {
            view.pending = None;
            view.message = message;
        }
    }

    /// The answer to the view's request.
    pub(in crate::app) fn value_event(&mut self, ctx: &egui::Context, kind: EventKind) {
        let values = &mut self.database.document.tools.values;
        values.ticket = None;
        let Some(view) = values.view.as_mut() else {
            return;
        };
        view.pending = None;
        let ours = view.source.handle();
        let mut copy = None;
        match kind {
            EventKind::ValueChunk {
                handle,
                offset,
                bytes,
                ..
            } if ours == Some(handle) => {
                view.chunks.insert(offset / CHUNK, Arc::from(bytes));
            }
            EventKind::ValueLoaded { handle, bytes } if ours == Some(handle) => {
                view.loaded = Some(Arc::from(bytes));
                copy = view.copy.take();
            }
            EventKind::ValueFound { handle, offset } if ours == Some(handle) => found(view, offset),
            EventKind::ValueSaved(path) => view.message = format!("Saved to {path}"),
            EventKind::Failed(error) => {
                let message = error.to_string();
                view.expired |= message.contains("expired");
                view.copy = None;
                view.message = if view.expired {
                    "The value is no longer held · refresh to read it again".into()
                } else {
                    message
                };
            }
            _ => {}
        }
        if let Some((format, at, len)) = copy {
            self.copy_range(ctx, format, at, len);
        }
    }
}

/// Select a match, or say there is none after the cursor.
fn found(view: &mut View, offset: Option<u64>) {
    match offset {
        Some(at) => {
            view.found = Some(at);
            view.select(at, view.needle.len() as u64);
            view.message.clear();
        }
        None => view.message = "No match after the cursor".into(),
    }
}

/// Placeholder rows while the whole value loads for a lens, or why it cannot.
fn loading(ui: &mut egui::Ui, view: &mut View) {
    if view.source.handle().is_none() || view.source.len() > LOAD {
        let text = "Over the 16 MiB a lens reads · use hex or save to file";
        return icons::empty_state(ui, Icon::Warning, text);
    }
    if view.pending.is_none() && view.loaded.is_none() {
        view.want = Some(Want::Load);
    }
    let rect = ui.available_rect_before_wrap();
    let color = crate::appearance::Tokens::of(ui.ctx()).raised;
    for i in 0..((rect.height() / hex::LINE) as usize).min(12) {
        let line = egui::Rect::from_min_size(
            rect.min + egui::vec2(4.0, i as f32 * hex::LINE),
            egui::vec2(rect.width() - 8.0, hex::LINE),
        );
        presentation::placeholder_row(ui.painter(), line, i, color);
    }
    let response = ui.allocate_rect(rect, egui::Sense::hover());
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::Role::ProgressIndicator, true, "Loading value")
    });
}

/// The cursor's offset (click to go to another), the selection's size, the
/// readings, and the byte order.
fn footer_readings(ui: &mut egui::Ui, view: &mut View, wide: bool) {
    let tokens = crate::appearance::Tokens::of(ui.ctx());
    let mono = |text: String, color| {
        egui::RichText::new(text)
            .monospace()
            .size(11.0)
            .color(color)
    };
    let offset = ui
        .add(
            egui::Button::new(mono(
                inspect::offset_text(view.cursor),
                ui.visuals().text_color(),
            ))
            .frame(false),
        )
        .on_hover_text("Go to offset · Ctrl G");
    if offset.clicked() {
        view.goto = Some(String::new());
    }
    ui.label(mono(format!("{} B", view.selected()), tokens.faint));
    for (label, value) in inspect::readings(view, wide) {
        ui.label(mono(label.into(), tokens.faint));
        ui.label(mono(value, ui.visuals().text_color()));
    }
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        let (text, other) = match view.order {
            Order::Little => ("LE", Order::Big),
            Order::Big => ("BE", Order::Little),
        };
        let chip = ui.add(egui::Button::new(mono(text.into(), ui.visuals().text_color())).small());
        if chip.on_hover_text("Byte order · click to swap").clicked() {
            view.order = other;
        }
    });
}

/// The image's colour and frames, then the zoom: the Relationships tile's
/// − percentage + control, and 1:1.
fn image_footer(
    ui: &mut egui::Ui,
    view: &mut View,
    (meta, size, scale): (lens::Meta, egui::Vec2, f32),
) {
    let tokens = crate::appearance::Tokens::of(ui.ctx());
    let text = |text: String| egui::RichText::new(text).size(11.5);
    let about = format!(
        "{} × {}  {}  {}",
        meta.width,
        meta.height,
        meta.color,
        meta.frames()
    );
    ui.label(text(about).color(tokens.faint));
    // The texture may be scaled down from the image: zoom is shown against the image.
    let actual = |scale: f32| scale * size.x / meta.width.max(1) as f32;
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        if ui
            .add(egui::Button::new(text("1:1".into())).small())
            .on_hover_text("Actual size")
            .clicked()
        {
            view.zoom = Some(meta.width as f32 / size.x.max(1.0));
            view.pan = egui::Vec2::ZERO;
        }
        if icons::line_control(ui, Icon::Plus, "Zoom in", "", None).clicked() {
            view.zoom = Some(scale * 1.25);
        }
        let percent = format!("{:.0}%", actual(scale) * 100.0);
        let fit = ui.add(egui::Button::new(text(percent)).frame(false));
        if fit.on_hover_text("Current zoom · click to fit").clicked() {
            view.zoom = None;
            view.pan = egui::Vec2::ZERO;
        }
        if icons::line_control(ui, Icon::Minimize, "Zoom out", "", None).clicked() {
            view.zoom = Some(scale * 0.8);
        }
    });
}

/// An icon control in `rect` on the header line; returns whether it was clicked.
fn line_button(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    icon: Icon,
    name: &str,
    shortcut: &str,
    selected: Option<bool>,
) -> bool {
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .id_salt(("value-tool", name))
            .max_rect(rect),
    );
    icons::line_control(&mut child, icon, name, shortcut, selected).clicked()
}

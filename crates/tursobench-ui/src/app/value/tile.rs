//! The record tile's second level: one field filling the tile body, read
//! through a lens, under a header that is the field's own row.
use super::{
    Decoded, Lens, Offer, Summary, Target, hex, inspect,
    job::Work,
    lens::{self, Shown, Size},
    view::{CHUNK, CLIPBOARD, LOAD, Source, View},
};
use crate::app::{
    App,
    icons::{self, Icon},
    presentation,
};
use eframe::egui;
use std::sync::Arc;
use tursobench_blob::{
    Kind, Order, encode::Format as CopyFormat, formats::Format, inspect::real, tree::Part,
};
use tursobench_core::Value;

/// The tile width from which the hex lens adds its inspector list.
const EXPANDED: f32 = 820.0;
/// The width of the expanded tile's entropy strip and inspector list.
const SIDE: f32 = 250.0;

/// What the header asked for.
enum Header {
    Back,
    Lens(Lens),
    Find,
    Copy(CopyFormat),
    CopyImage,
    Save(&'static str),
    Load,
}

/// What the hex lens's menu asked for.
enum Edit {
    Null,
    Insert,
    Delete,
}

impl App {
    pub(in crate::app) fn value_open(&self) -> bool {
        self.database.document.tools.values.view.is_some()
    }

    /// Open field `col` of `row` in the value view, filling the record tile.
    pub(in crate::app) fn open_value(&mut self, row: usize, col: usize) {
        let document = &self.database.document;
        let from = (document.result.current, document.draft_revision);
        let Some(view) = self
            .field_value(row, col)
            .and_then(|value| View::new(row, col, value, from))
        else {
            return;
        };
        self.open_record_window(row, col);
        // Table data opens the value in the record tile; any other result
        // keeps the detached inspector, which shows it instead.
        if self.database.document.kind == crate::app::tiling::Kind::Data {
            self.database.document.ui.inspector_detached = false;
        }
        self.database.document.tools.values.view = Some(view);
    }
    /// Whether the open value shows in the detached inspector rather than
    /// the record tile, which shows table data only.
    pub(in crate::app) fn value_in_dialog(&self) -> bool {
        self.value_open() && self.database.document.kind != crate::app::tiling::Kind::Data
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
        // A staged edit of the same field keeps the place in the value.
        let same = row == Some(view.row);
        let place = (
            view.edit_cursor.unwrap_or(view.cursor),
            view.top,
            view.insert,
            view.half,
            (view.per, view.lines),
        );
        let next = row
            .and_then(|row| Some((row, self.field_value(row, col)?)))
            .and_then(|(row, value)| View::new(row, col, value, from));
        let values = &mut self.database.document.tools.values;
        values.ticket = None;
        values.view = next.map(|mut view| {
            (view.order, view.characters) = (order, characters);
            if same {
                // A staged edit keeps the lens it was made in, and the
                // geometry as last painted, so the cursor stays in view.
                view.lens = lens;
                (view.top, view.insert, (view.per, view.lines)) = (place.1, place.2, place.4);
                view.edit_at(place.0);
                view.half = place.3;
            }
            view
        });
        values.view.is_some()
    }

    /// The kind and lens of the open value. The lens is kept once shown, so
    /// an edit that changes what the bytes look like does not change it.
    fn value_lens(&mut self, ctx: &egui::Context) -> Option<Offer> {
        let view = self.database.document.tools.values.view.as_ref()?;
        let (row, col, pinned) = (view.row, view.col, view.lens);
        let mut offer = self.value_offer(ctx, row, col)?;
        match pinned {
            Some(lens) if offer.lenses.contains(&lens) => offer.lens = lens,
            _ => {
                if let Some(view) = self.value_view() {
                    view.lens = Some(offer.lens);
                }
            }
        }
        Some(offer)
    }

    /// The lens the open value is shown through now.
    #[cfg(test)]
    pub(in crate::app) fn value_lens_now(&mut self, ctx: &egui::Context) -> Option<Lens> {
        self.value_lens(ctx).map(|offer| offer.lens)
    }

    /// The value view: header, body through the lens, and footer.
    /// `keys` says whether the surface showing it takes the keyboard.
    pub(in crate::app) fn value_view_ui(&mut self, ui: &mut egui::Ui, keys: bool) {
        if !self.sync_value_view() {
            return;
        }
        self.poll_value_save();
        self.poll_value_load();
        self.poll_value_copy();
        let Some(offer) = self.value_lens(ui.ctx()) else {
            self.close_value();
            return;
        };
        let (kind, lens) = (offer.kind, offer.lens);
        let editable = self.value_refusal().is_none();
        if let Some(header) = self.value_header(ui, &offer, editable) {
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
            Lens::Hex => {
                if let Some(edit) = self.hex_body(ui, editable) {
                    self.value_edit(edit);
                }
            }
            Lens::Text => self.text_body(ui, kind),
            Lens::Tree => self.tree_body(ui, offer.format),
            Lens::Image => self.image_body(ui),
            Lens::Vector => self.vector_body(ui),
            Lens::Contents => self.contents_body(ui, kind),
        }
        if keys {
            self.value_keys(ui.ctx(), lens);
        }
        self.drive_value();
    }

    /// The field's own row as the header: back, name, summary and tools.
    fn value_header(&self, ui: &mut egui::Ui, offer: &Offer, editable: bool) -> Option<Header> {
        let lens = offer.lens;
        let view = self.database.document.tools.values.view.as_ref()?;
        let name = self
            .database
            .document
            .result
            .columns
            .get(view.col)?
            .name
            .as_str();
        let header = self.database.document.column_header(view.col);
        let vector = super::vector_hint(&header.column.name, header.declared_type());
        let summary = self
            .field_value(view.row, view.col)
            .and_then(|value| Summary::for_column(ui.ctx(), value, vector));
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
        let width = presentation::text_width(ui, name, 12.5) + 8.0;
        let label =
            egui::Rect::from_min_size(rect.min + egui::vec2(22.0, 0.0), egui::vec2(width, 24.0));
        presentation::text_at(ui, label, name, ui.visuals().text_color(), 12.5);
        // Tools from the right: save, copy as, find, then the lenses.
        let mut right = rect.right();
        let mut slot = |gap: f32| {
            right -= 24.0 + gap;
            egui::Rect::from_min_size(egui::pos2(right, rect.top()), egui::vec2(24.0, 24.0))
        };
        if editable && line_button(ui, slot(0.0), Icon::Import, "Replace from file…", "", None) {
            action = Some(Header::Load);
        }
        if line_button(ui, slot(0.0), Icon::Export, "Save to file…", "", None) {
            action = Some(Header::Save(offer.kind.extension()));
        }
        action = Self::copy_menu(ui, slot(0.0), lens == Lens::Image).or(action);
        if lens == Lens::Hex && line_button(ui, slot(0.0), Icon::Search, "Find", "Ctrl F", None) {
            action = Some(Header::Find);
        }
        let offered = &offer.lenses;
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
    fn copy_menu(ui: &mut egui::Ui, rect: egui::Rect, image: bool) -> Option<Header> {
        let mut child = ui.new_child(egui::UiBuilder::new().id_salt("value-copy").max_rect(rect));
        let response = icons::line_control(&mut child, Icon::Copy, "Copy as…", "", None);
        let mut chosen = None;
        egui::Popup::menu(&response).show(|ui| {
            if image && ui.button("Image").clicked() {
                chosen = Some(Header::CopyImage);
            }
            for format in CopyFormat::ALL {
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
                if let Some(view) = self.value_view() {
                    view.lens = Some(lens);
                }
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
            Header::CopyImage => {
                self.value_image(ctx, Size::Copy);
            }
            Header::Save(extension) => self.save_value(ctx, extension),
            Header::Load => self.replace_value_from_file(ctx),
        }
    }

    /// The paged hex lens, with click and drag selection, the wheel and a scroll bar.
    fn hex_body(&mut self, ui: &mut egui::Ui, editable: bool) -> Option<Edit> {
        let color = ui.visuals().text_color();
        let rect = ui.available_rect_before_wrap();
        let document = &mut self.database.document;
        let view = document.tools.values.view.as_mut()?;
        // Bytes that differ from the stored value are staged changes.
        let stored = match document
            .result
            .rows
            .get(view.row)
            .and_then(|r| r.get(view.col))
        {
            Some(Value::Blob(bytes)) => Some(bytes.as_slice()),
            _ => None,
        };
        // An expanded tile adds an entropy strip and the full inspector list.
        let side = (rect.width() >= EXPANDED).then(|| {
            egui::Rect::from_min_max(egui::pos2(rect.right() - SIDE, rect.top()), rect.max)
        });
        let main = side.map_or(rect, |side| {
            egui::Rect::from_min_max(rect.min, egui::pos2(side.left(), rect.bottom()))
        });
        let body = egui::Rect::from_min_max(
            main.min + egui::vec2(4.0, 2.0),
            main.max - egui::vec2(12.0, 2.0),
        );
        let grid = hex::Grid::new(ui, body, view.source.len());
        (view.per, view.lines) = (grid.per, grid.lines());
        let response = ui.interact(
            body,
            ui.scope_id().with("value-hex"),
            egui::Sense::click_and_drag(),
        );
        hex_pointer(ui, &grid, &response, view);
        let total = view.source.len().div_ceil(grid.per as u64).max(1);
        let track = egui::Rect::from_min_max(
            egui::pos2(main.right() - 8.0, body.top()),
            egui::pos2(main.right() - 4.0, body.bottom()),
        );
        if let Some(line) = hex::scrollbar(ui, track, view.top, grid.lines(), total) {
            view.scroll_to_line(line);
        }
        let changed =
            |at: u64| stored.is_some_and(|s| s.get(at as usize) != view.byte(at).as_ref());
        hex::paint(ui, &grid, view, color, &changed);
        if let Some(side) = side {
            let strip =
                egui::Rect::from_x_y_ranges(side.left() + 2.0..=side.left() + 12.0, body.y_range());
            if let Some(line) = inspect::minimap(ui, strip, view) {
                view.scroll_to_line(line);
            }
            let list =
                egui::Rect::from_min_max(egui::pos2(side.left() + 18.0, side.top()), side.max);
            inspect::list(ui, list, view);
        }
        let label = format!("Byte {} of {}", view.cursor, view.source.len());
        response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Grid, true, &label));
        let edit = hex_menu(&response, editable, view.insert);
        ui.allocate_rect(rect, egui::Sense::hover());
        edit
    }

    /// Apply an edit chosen from the hex lens's menu.
    fn value_edit(&mut self, edit: Edit) {
        match edit {
            Edit::Null => self.stage_value_null(),
            Edit::Insert => {
                if let Some(view) = self.value_view() {
                    view.insert = !view.insert;
                }
            }
            Edit::Delete => self.delete_value_selection(),
        }
    }

    /// The text lens: the value's text, wrapped, from memory or loaded whole.
    fn text_body(&mut self, ui: &mut egui::Ui, kind: Kind) {
        if let Kind::Geometry { .. } = kind {
            if let Some(Decoded::Text(wkt)) =
                self.decode_open(ui, Lens::Text, |b| Some(Work::Wkt(b)))
            {
                ui.add(
                    egui::Label::new(egui::RichText::new(wkt.as_ref()).monospace().size(12.0))
                        .wrap(),
                );
                if let Some(view) = self.value_view() {
                    (view.shown, view.note) = (Some(wkt), String::new());
                }
            }
            return;
        }
        let Some(view) = self.value_view() else {
            return;
        };
        let Some(bytes) = view.whole() else {
            return loading(ui, view);
        };
        let encoding = match kind {
            Kind::Text(encoding) if !view.text => Some(encoding),
            _ => None,
        };
        let (text, cut) = view.drawn.text(&bytes, encoding);
        // Say when the text stops short of the value, and where the rest is.
        view.note = if cut {
            let size = super::summary::byte_size(view.source.len());
            format!("First 1 MiB of {size} · the hex lens reads the rest")
        } else {
            String::new()
        };
        view.shown = Some(text.clone());
        egui::ScrollArea::vertical()
            .id_salt("value-text")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.add(egui::Label::new(egui::RichText::new(text.as_ref()).size(12.5)).wrap())
            });
    }

    /// The decode of the open value for `lens`, from the whole value once in memory.
    fn decode_open(
        &mut self,
        ui: &mut egui::Ui,
        lens: Lens,
        work: impl FnOnce(Arc<[u8]>) -> Option<Work>,
    ) -> Option<Decoded> {
        let document = &mut self.database.document;
        let values = &mut document.tools.values;
        let view = values.view.as_mut()?;
        let (row, col) = (view.row, view.col);
        let Some(bytes) = view.whole() else {
            loading(ui, view);
            return None;
        };
        let target = Target {
            result: document.result.current,
            drafts: document.draft_revision,
            row,
            col,
            whole: true,
        };
        match values.decode(ui.ctx(), target, lens, || work(bytes)) {
            Some(Decoded::Pending) => {
                icons::empty_state(ui, lens.icon(), "Reading");
                None
            }
            Some(Decoded::Failed(reason)) => {
                icons::empty_state(
                    ui,
                    lens.icon(),
                    &format!("Not readable through {}: {reason}", lens.name()),
                );
                None
            }
            None => {
                icons::empty_state(
                    ui,
                    lens.icon(),
                    &format!("Not readable through {}", lens.name()),
                );
                None
            }
            decoded => decoded,
        }
    }

    /// The tree lens: the whole value decoded off the UI thread, printed as JSON.
    fn tree_body(&mut self, ui: &mut egui::Ui, format: Option<Format>) {
        let work = |bytes| format.map(|format| Work::Tree(bytes, format));
        if let Some(Decoded::Tree(tree)) = self.decode_open(ui, Lens::Tree, work)
            && let Some(view) = self.value_view()
        {
            tree_rows(ui, &view.drawn.lines(&tree), tree.complete);
        }
    }

    /// The vector lens: bars across the tile, the norm and range, then every value.
    fn vector_body(&mut self, ui: &mut egui::Ui) {
        let Some(Decoded::Vector(vector)) =
            self.decode_open(ui, Lens::Vector, |b| Some(Work::Vector(b)))
        else {
            return;
        };
        let Some(view) = self.value_view() else {
            return;
        };
        let tokens = crate::appearance::Tokens::of(ui.ctx());
        let (rect, _) =
            ui.allocate_exact_size(egui::vec2(ui.available_width(), 72.0), egui::Sense::hover());
        let chart = rect.shrink2(egui::vec2(4.0, 6.0));
        let (numbers, (norm, min, max), bars) =
            view.drawn.vector(&vector, (chart.width() / 3.0) as usize);
        super::preview::bars(ui, ui.painter(), chart, &bars);
        let stats = format!(
            "{} × {}   ‖v‖ {}   min {}   max {}",
            vector.layout.name(),
            vector.dims,
            real(norm),
            real(min),
            real(max)
        );
        ui.label(
            egui::RichText::new(stats)
                .monospace()
                .size(11.0)
                .color(tokens.faint),
        );
        let plain = ui.visuals().text_color();
        egui::ScrollArea::vertical()
            .id_salt("value-vector")
            .auto_shrink([false, false])
            .show_rows(ui, hex::LINE, numbers.len(), |ui, range| {
                ui.spacing_mut().item_spacing.y = 0.0;
                for (i, number) in numbers
                    .iter()
                    .enumerate()
                    .skip(range.start)
                    .take(range.len())
                {
                    let line = format!("[{i}]  {}", real(*number));
                    let font = super::preview::font(ui);
                    text_row(
                        ui,
                        egui::text::LayoutJob::simple_singleline(line, font, plain),
                    );
                }
            });
    }

    /// The contents lens: inflated off the UI thread, read again as a tree,
    /// text or bytes.
    fn contents_body(&mut self, ui: &mut egui::Ui, kind: Kind) {
        let work = |bytes| match kind {
            Kind::Compressed(compression) => Some(Work::Contents(bytes, compression)),
            _ => None,
        };
        let Some(Decoded::Contents(contents)) = self.decode_open(ui, Lens::Contents, work) else {
            return;
        };
        let Some(view) = self.value_view() else {
            return;
        };
        let inner = Summary::describe_kind(contents.kind);
        let first = if contents.truncated {
            " · first 16 MiB of contents"
        } else {
            ""
        };
        view.note = format!(
            "{} › {inner} · {}{first}",
            contents.compression.name(),
            super::summary::byte_size(contents.len as u64)
        );
        if let Some(tree) = &contents.tree {
            return tree_rows(ui, &view.drawn.lines(tree), tree.complete);
        }
        if let Some(text) = &contents.text {
            egui::ScrollArea::vertical()
                .id_salt("value-contents")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.add(egui::Label::new(egui::RichText::new(text.as_str()).size(12.5)).wrap())
                });
            return;
        }
        let per = 16;
        let tokens = crate::appearance::Tokens::of(ui.ctx());
        let color = ui.visuals().text_color();
        let lines = contents.bytes.len().div_ceil(per);
        egui::ScrollArea::vertical()
            .id_salt("value-contents-hex")
            .auto_shrink([false, false])
            .show_rows(ui, hex::LINE, lines, |ui, range| {
                ui.spacing_mut().item_spacing.y = 0.0;
                for line in range {
                    let chunk =
                        &contents.bytes[line * per..((line + 1) * per).min(contents.bytes.len())];
                    let job = super::preview::hex_line(
                        ui,
                        (line * per) as u64,
                        6,
                        chunk,
                        per,
                        color,
                        tokens.faint,
                    );
                    text_row(ui, job);
                }
            });
    }

    /// The value's image, decoded off the UI thread: fitted, or at the
    /// footer's zoom, and moved by dragging.
    fn image_body(&mut self, ui: &mut egui::Ui) {
        let (id, size, meta) = match self.value_image(ui.ctx(), Size::Full) {
            Some(Shown::Ready(id, size, meta)) => (id, size, meta),
            Some(Shown::Pending) => return icons::empty_state(ui, Icon::Image, "Reading"),
            Some(Shown::Failed(reason)) => return icons::empty_state(ui, Icon::Image, &reason),
            None => {
                if let Some(view) = self.value_view() {
                    loading(ui, view);
                }
                return;
            }
        };
        let Some(view) = self.value_view() else {
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

    /// Request the image for display or for the clipboard.
    fn value_image(&mut self, ctx: &egui::Context, size: Size) -> Option<Shown> {
        let document = &mut self.database.document;
        let values = &mut document.tools.values;
        let view = values.view.as_mut()?;
        let bytes = view.whole()?;
        if size == Size::Copy {
            view.message = "Reading the image to copy".into();
        }
        let target = Target {
            result: document.result.current,
            drafts: document.draft_revision,
            row: view.row,
            col: view.col,
            whole: true,
        };
        Some(values.images.get(ctx, target, size, || {
            match tursobench_blob::sniff(&bytes, bytes.len() as u64) {
                Kind::Image { format, .. } => Some((bytes, format)),
                _ => None,
            }
        }))
    }

    /// The footer: offset, selection and readings, or the find or go-to input.
    fn value_footer(&mut self, ui: &mut egui::Ui, lens: Lens) {
        let wide = ui.available_width() >= 480.0;
        let expanded = ui.available_width() >= EXPANDED;
        let tokens = crate::appearance::Tokens::of(ui.ctx());
        let Some(view) = self.value_view() else {
            return;
        };
        ui.horizontal_centered(|ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
            if view.find.is_some() {
                return find_input(ui, view);
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
                let note = if matches!(lens, Lens::Contents | Lens::Text) && !view.note.is_empty() {
                    view.note.clone()
                } else {
                    super::summary::byte_size(view.source.len())
                };
                ui.label(egui::RichText::new(note).size(11.5).color(tokens.faint));
                return;
            }
            footer_readings(ui, view, wide, expanded);
        });
    }

    /// Keys while a value is open and nothing else takes them. Finding,
    /// moving and editing act on the hex lens's cursor, so only that lens
    /// takes them; elsewhere a copy takes the whole value.
    fn value_keys(&mut self, ctx: &egui::Context, lens: Lens) {
        if egui::Popup::is_any_open(ctx) || self.database.ui.palette_open {
            return;
        }
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter)) {
            return self.submit_value_input(ctx);
        }
        if ctx.egui_wants_keyboard_input() {
            return;
        }
        let copy = ctx.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::Copy)));
        if lens == Lens::Hex {
            self.hex_keys(ctx, copy);
        } else if lens == Lens::Text && copy {
            self.copy_shown_text(ctx);
        }
    }
    fn copy_shown_text(&mut self, ctx: &egui::Context) {
        let Some(view) = self.value_view() else {
            return;
        };
        let Some(text) = &view.shown else {
            return;
        };
        if text.len() > CLIPBOARD {
            view.message = "Over the 2 MiB clipboard budget · save to file instead".into();
        } else {
            ctx.copy_text(text.to_string());
            view.message = "Copied the text shown".into();
        }
    }
    fn hex_keys(&mut self, ctx: &egui::Context, copy: bool) {
        let pressed = |key| ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, key));
        if pressed(egui::Key::F) {
            return self.header_action(ctx, Header::Find);
        }
        if self.value_edit_keys(ctx) {
            return;
        }
        if pressed(egui::Key::G) {
            if let Some(view) = self.value_view() {
                view.goto = Some(String::new());
            }
            return;
        }
        if copy {
            let characters = self.value_view().is_some_and(|v| v.characters);
            let format = if characters {
                CopyFormat::Text
            } else {
                CopyFormat::Hex
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

    /// Say whether the image was copied, once its decode is done.
    fn poll_value_copy(&mut self) {
        let values = &mut self.database.document.tools.values;
        if let Some(outcome) = values.images.copied.take()
            && let Some(view) = values.view.as_mut()
        {
            view.message = match outcome {
                Ok(()) => "Copied the image".into(),
                Err(reason) => format!("Could not copy the image: {reason}"),
            };
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
}

/// A click or drag on the hex lens moves the cursor or extends the
/// selection; the wheel scrolls it.
fn hex_pointer(ui: &egui::Ui, grid: &hex::Grid, response: &egui::Response, view: &mut View) {
    let pressed = response.clicked() || response.dragged() || response.drag_started();
    if let Some(pos) = response.interact_pointer_pos().filter(|_| pressed)
        && let Some((at, characters)) = grid.hit(pos, view.top)
    {
        let extend =
            response.dragged() && !response.drag_started() || ui.input(|i| i.modifiers.shift);
        view.characters = characters;
        view.move_to(at, extend);
    }
    let delta = ui.input(|i| i.smooth_scroll_delta.y);
    if response.hovered() && delta != 0.0 {
        view.scroll_to_line(view.top as i64 - (delta / hex::LINE).round() as i64);
    }
}

/// The hex lens's menu of edits, enabled when the value can be edited.
fn hex_menu(response: &egui::Response, editable: bool, insert: bool) -> Option<Edit> {
    let mut edit = None;
    response.context_menu(|ui| {
        ui.add_enabled_ui(editable, |ui| {
            let mode = if insert {
                "Overwrite bytes"
            } else {
                "Insert bytes"
            };
            for (label, chosen) in [
                (mode, Edit::Insert),
                ("Delete selected bytes", Edit::Delete),
                ("Set NULL", Edit::Null),
            ] {
                if ui.button(label).clicked() {
                    edit = Some(chosen);
                }
            }
        });
    });
    edit
}

/// The find input, with why the last search found nothing beside it.
fn find_input(ui: &mut egui::Ui, view: &mut View) {
    let tokens = crate::appearance::Tokens::of(ui.ctx());
    let message = view.message.clone();
    let hint = if view.characters {
        "Find text"
    } else {
        "Find bytes (00 ff), or \"text\""
    };
    let width = if message.is_empty() {
        ui.available_width() - 8.0
    } else {
        ui.available_width() * 0.55
    };
    let Some(query) = view.find.as_mut() else {
        return;
    };
    let input = egui::TextEdit::singleline(query)
        .hint_text(hint)
        .desired_width(width);
    let response = ui.add(input);
    response.request_focus();
    if response.changed() {
        view.message.clear();
    } else if !message.is_empty() {
        ui.label(egui::RichText::new(message).size(11.5).color(tokens.faint));
    }
}

/// Placeholder rows while the whole value loads for a lens, or why it cannot.
fn loading(ui: &mut egui::Ui, view: &mut View) {
    if view.source.handle().is_none() || view.source.len() > LOAD {
        let text = "Over the 16 MiB a lens reads · use hex or save to file";
        return icons::empty_state(ui, Icon::Warning, text);
    }
    if view.stalled && view.loaded.is_none() {
        return icons::empty_state(ui, Icon::Warning, "The value could not be read");
    }
    view.want_whole();
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
fn footer_readings(ui: &mut egui::Ui, view: &mut View, wide: bool, expanded: bool) {
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
        // Expanded, the list holds the byte order; the footer says how much is held.
        if expanded {
            let size = super::summary::byte_size(view.source.len());
            let held = match view.source {
                Source::Deferred { len, .. } => {
                    format!(
                        "{size} · {} of {} chunks held",
                        view.chunks.len(),
                        len.div_ceil(CHUNK)
                    )
                }
                Source::Bytes(_) => size,
            };
            ui.label(mono(held, tokens.faint));
            return;
        }
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

/// One line of a lens's list, from the left, cut with an ellipsis.
fn text_row(ui: &mut egui::Ui, mut job: egui::text::LayoutJob) {
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
    ui.painter().galley(at, galley, ui.visuals().text_color());
}

/// A tree's printed lines, in the SQL editor's colours, only those in view
/// drawn; `complete` is false when the tree stopped at its limits.
fn tree_rows(ui: &mut egui::Ui, lines: &[tursobench_blob::tree::Line], complete: bool) {
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
                text_row(ui, job);
            }
        });
    if !complete {
        let tokens = crate::appearance::Tokens::of(ui.ctx());
        ui.label(
            egui::RichText::new("Showing the first 10,000 nodes")
                .size(11.5)
                .color(tokens.faint),
        );
    }
}

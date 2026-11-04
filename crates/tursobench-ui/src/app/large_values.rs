use super::App;
use eframe::egui;
use tursobench_core::{DeferredCell, EventKind, LargeKind, Ticket};
use tursobench_db::{Operation, Request};

#[derive(Default)]
pub(super) struct Viewer {
    cell: Option<DeferredCell>,
    pub ticket: Option<Ticket>,
    offset: u64,
    data: Vec<u8>,
    path: String,
    search: String,
    message: String,
}
impl App {
    pub(super) fn open_large_value(&mut self, cell: DeferredCell) {
        self.document.tools.large = Viewer {
            cell: Some(cell),
            ..Default::default()
        };
        self.load_value_chunk();
    }
    fn value_request(&mut self, operation: Operation) {
        if self.document.tools.large.ticket.is_some() {
            return;
        }
        let Some(session) = &self.session else {
            self.document.tools.large.message = "The source session is closed".into();
            return;
        };
        self.sequence += 1;
        let ticket = session.ticket(self.sequence, self.generation);
        match session.submit(Request { ticket, operation }) {
            Ok(()) => self.document.tools.large.ticket = Some(ticket),
            Err(e) => self.document.tools.large.message = e.to_string(),
        }
    }
    fn load_value_chunk(&mut self) {
        let viewer = &self.document.tools.large;
        if let Some(handle) = viewer.cell.as_ref().and_then(|c| c.handle) {
            self.value_request(Operation::ReadValue {
                handle,
                offset: viewer.offset,
                bytes: 16 * 1024,
            });
        } else {
            self.document.tools.large.message =
                "Full value unavailable: it exceeded the spool limit or temporary storage failed"
                    .into();
        }
    }
    pub(super) fn value_event(&mut self, kind: EventKind) {
        let viewer = &mut self.document.tools.large;
        match kind {
            EventKind::ValueChunk {
                handle,
                offset,
                bytes,
                ..
            } if viewer
                .cell
                .as_ref()
                .is_some_and(|c| c.handle == Some(handle)) =>
            {
                viewer.offset = offset;
                viewer.data = bytes;
                viewer.message.clear();
            }
            EventKind::ValueSaved(path) => viewer.message = format!("Saved full value to {path}"),
            EventKind::Failed(e) => viewer.message = e.to_string(),
            _ => return,
        }
        viewer.ticket = None;
    }
    pub(super) fn large_value_ui(&mut self, ctx: &egui::Context) {
        let viewer = &mut self.document.tools.large;
        let Some(cell) = viewer.cell.clone() else {
            return;
        };
        let mut open = true;
        let mut load = false;
        let mut save = false;
        egui::Window::new("Full value").id(egui::Id::unique(("large-value",self.document.id))).open(&mut open).default_width(660.0).vscroll(true).show(ctx,|ui|{
            ui.label(format!("{:?} · {} bytes · immutable result snapshot",cell.kind,cell.bytes));
            ui.horizontal(|ui|{
                ui.label("Byte offset");
                ui.add(egui::DragValue::new(&mut viewer.offset).range(0..=cell.bytes));
                load=ui.add_enabled(viewer.ticket.is_none(),egui::Button::new("Load 16 KiB")).clicked();
            });
            let text=if cell.kind==LargeKind::Text {String::from_utf8_lossy(&viewer.data).into_owned()}else{crate::export::hex(&viewer.data)};
            ui.label("Find in this chunk");ui.text_edit_singleline(&mut viewer.search);
            if !viewer.search.is_empty(){ui.label(text.find(&viewer.search).map_or_else(||"No match in chunk".into(),|i|format!("Match at preview position {i}")));}
            super::results::code_view(ui,&text,12);
            ui.small("Chunk boundaries may split a UTF-8 character. Saving preserves the exact original bytes.");
            ui.text_edit_singleline(&mut viewer.path);
            save=ui.add_enabled(viewer.ticket.is_none()&&!viewer.path.is_empty()&&cell.handle.is_some(),egui::Button::new("Save full value to new file")).clicked();
            if viewer.ticket.is_some(){ui.spinner();}
            if !viewer.message.is_empty(){ui.label(&viewer.message);}
        });
        if !open {
            self.document.tools.large.cell = None;
            return;
        }
        if load {
            self.load_value_chunk();
        }
        if save && let Some(handle) = cell.handle {
            self.value_request(Operation::SaveValue {
                handle,
                path: self.document.tools.large.path.clone().into(),
            });
        }
    }
}

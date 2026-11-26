//! Fluxos de arquivo da interface: abrir, salvar, confirmar e avisar.
//!
//! É um submódulo de `tui` para poder usar o estado privado de `AsciiUi`. Nada aqui
//! depende do egui além das teclas: os efeitos "de janela" (fechar) viram flags
//! (`want_close`) que `AsciiUi::show` aplica, o que deixa os fluxos testáveis sem
//! abrir janela.

use super::{AsciiUi, Drag, Layout, Look, Pointer};
use crate::buffer::Buffer;
use crate::dialogs::{MsgBox, Picker, PickerHit, PickerLayout, PickerMode, PickerOutcome};
use crate::files::{self, FileFormat};
use crate::menu::Cmd;
use crate::screen::Screen;
use crate::scrollbar::BarHit;
use eframe::egui::Key;
use std::path::{Path, PathBuf};

/// O que fazer depois de resolver as alterações não salvas.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Pending {
    New,
    Open,
    Quit,
}

/// Perguntas e avisos (caixas de mensagem).
pub(super) enum Ask {
    /// "Salvar / Descartar / Cancelar" antes de executar `Pending`.
    Unsaved(Pending),
    /// "Sobrescrever?": guarda o seletor para voltar a ele se a resposta for "Não".
    Overwrite {
        picker: Picker,
        path: PathBuf,
        then: Option<Pending>,
    },
    /// Só um aviso, com botão OK.
    Notice,
}

/// Diálogo aberto no momento (no máximo um).
pub(super) enum Modal {
    None,
    About,
    Picker {
        picker: Picker,
        /// O que continuar depois de salvar (quando o seletor veio de "alterações
        /// não salvas").
        then: Option<Pending>,
    },
    Ask {
        dlg: MsgBox,
        ask: Ask,
    },
}

/// Arquivo em edição: de onde veio (e para onde vai ao salvar) e como era no disco.
pub(super) struct FileState {
    pub path: Option<PathBuf>,
    pub format: FileFormat,
}

impl FileState {
    pub fn untitled() -> Self {
        Self {
            path: None,
            format: FileFormat::default(),
        }
    }

    /// Nome para mostrar no título e nas mensagens.
    pub fn display_name(&self) -> String {
        match &self.path {
            Some(path) => files::file_label(path),
            None => "sem-titulo".to_string(),
        }
    }

    /// Pasta inicial dos seletores.
    pub fn dir(&self) -> PathBuf {
        match &self.path {
            Some(path) => files::dir_of(path),
            None => files::start_dir(),
        }
    }

    /// Nome sugerido em "Salvar como".
    pub fn suggested_name(&self) -> String {
        match &self.path {
            Some(path) => files::file_label(path),
            None => "sem-titulo.rs".to_string(),
        }
    }
}

impl AsciiUi {
    // --------------------------------------------------------- comandos de arquivo

    /// Novo, Abrir, Salvar, Salvar como e Sair (os demais comandos são da `tui`).
    pub(super) fn file_cmd(&mut self, buf: &mut Buffer, cmd: Cmd) {
        match cmd {
            Cmd::New => self.request(buf, Pending::New),
            Cmd::Open => self.request(buf, Pending::Open),
            Cmd::Save => self.save(buf, None),
            Cmd::SaveAs => self.open_picker(PickerMode::SaveAs, None),
            Cmd::Quit => self.request(buf, Pending::Quit),
            _ => {}
        }
    }

    /// Executa `pending`; se houver alterações não salvas, pergunta antes.
    pub(super) fn request(&mut self, buf: &mut Buffer, pending: Pending) {
        if buf.is_dirty() {
            let dlg = MsgBox::unsaved(&self.file.display_name());
            self.modal = Modal::Ask {
                dlg,
                ask: Ask::Unsaved(pending),
            };
        } else {
            self.proceed(buf, pending);
        }
    }

    fn proceed(&mut self, buf: &mut Buffer, pending: Pending) {
        match pending {
            Pending::New => {
                self.replace_document(buf, "", FileState::untitled());
                self.notify("Novo documento".to_string());
            }
            Pending::Open => self.open_picker(PickerMode::Open, None),
            Pending::Quit => self.want_close = true,
        }
    }

    fn replace_document(&mut self, buf: &mut Buffer, text: &str, file: FileState) {
        *buf = Buffer::new(text);
        self.file = file;
        self.scroll_x = 0;
        self.scroll_y = 0;
    }

    fn open_picker(&mut self, mode: PickerMode, then: Option<Pending>) {
        let name = match mode {
            PickerMode::Open => String::new(),
            PickerMode::SaveAs => self.file.suggested_name(),
        };
        let picker = Picker::new(mode, self.file.dir(), &name);
        self.modal = Modal::Picker { picker, then };
    }

    /// Salvar. Sem nome ainda, vira "Salvar como". `then` é o que fazer depois.
    fn save(&mut self, buf: &mut Buffer, then: Option<Pending>) {
        let Some(path) = self.file.path.clone() else {
            self.open_picker(PickerMode::SaveAs, then);
            return;
        };
        match self.try_save(buf, &path) {
            Ok(()) => {
                if let Some(pending) = then {
                    self.proceed(buf, pending);
                }
            }
            Err(message) => self.show_notice("Erro ao salvar", &message),
        }
    }

    fn try_save(&mut self, buf: &mut Buffer, path: &Path) -> Result<(), String> {
        files::save(path, &buf.text(), self.file.format)?;
        buf.mark_saved();
        self.file.path = Some(path.to_path_buf());
        self.notify(format!("Salvo: {}", files::file_label(path)));
        Ok(())
    }

    /// Lê `path` para dentro do editor. Se falhar, o documento atual fica intacto.
    fn load_into(&mut self, buf: &mut Buffer, path: PathBuf) -> Result<(), String> {
        let loaded = files::load(&path)?;
        let label = files::file_label(&path);
        let file = FileState {
            path: Some(path),
            format: loaded.format,
        };
        self.replace_document(buf, &loaded.text, file);
        let mut message = format!("Aberto: {label}");
        if loaded.tabs {
            message.push_str(" (tabs convertidos em 4 espaços)");
        }
        self.notify(message);
        Ok(())
    }

    /// Abre `path` ao iniciar (argumento de linha de comando). Se o arquivo não existe,
    /// começa um documento vazio com esse nome, como `nano novo.txt`.
    pub fn open_initial(&mut self, buf: &mut Buffer, path: PathBuf) {
        let path = files::absolute(&path);
        if !path.exists() {
            let label = files::file_label(&path);
            let file = FileState {
                path: Some(path),
                format: FileFormat::default(),
            };
            self.replace_document(buf, "", file);
            self.notify(format!("Novo arquivo: {label}"));
        } else if let Err(message) = self.load_into(buf, path) {
            self.notify(format!("Erro ao abrir: {message}"));
        }
    }

    fn notify(&mut self, text: String) {
        self.message = Some((text, self.now + 5.0));
    }

    fn show_notice(&mut self, title: &str, message: &str) {
        self.modal = Modal::Ask {
            dlg: MsgBox::notice(title, message),
            ask: Ask::Notice,
        };
    }

    // ------------------------------------------------------------ respostas

    fn picker_outcome(
        &mut self,
        buf: &mut Buffer,
        picker: Picker,
        then: Option<Pending>,
        outcome: PickerOutcome,
    ) {
        match outcome {
            PickerOutcome::Stay => self.modal = Modal::Picker { picker, then },
            PickerOutcome::Cancel => {}
            PickerOutcome::Chosen(path) => self.picker_chosen(buf, picker, then, path),
        }
    }

    /// A pessoa escolheu `path` no seletor. Erros ficam dentro do seletor, para ela
    /// poder tentar outro arquivo sem recomeçar.
    fn picker_chosen(
        &mut self,
        buf: &mut Buffer,
        mut picker: Picker,
        then: Option<Pending>,
        path: PathBuf,
    ) {
        match picker.mode() {
            PickerMode::Open => {
                if let Err(message) = self.load_into(buf, path) {
                    picker.set_error(message);
                    self.modal = Modal::Picker { picker, then };
                }
            }
            PickerMode::SaveAs => {
                let same_file = self.file.path.as_deref() == Some(path.as_path());
                if path.exists() && !same_file {
                    let dlg = MsgBox::overwrite(&files::file_label(&path));
                    self.modal = Modal::Ask {
                        dlg,
                        ask: Ask::Overwrite { picker, path, then },
                    };
                } else {
                    match self.try_save(buf, &path) {
                        Ok(()) => {
                            if let Some(pending) = then {
                                self.proceed(buf, pending);
                            }
                        }
                        Err(message) => {
                            picker.set_error(message);
                            self.modal = Modal::Picker { picker, then };
                        }
                    }
                }
            }
        }
    }

    /// Um botão de uma caixa de mensagem foi acionado.
    fn ask_pressed(&mut self, buf: &mut Buffer, ask: Ask, button: usize) {
        match ask {
            Ask::Notice => {}
            // 0 = Salvar, 1 = Descartar, 2 = Cancelar
            Ask::Unsaved(pending) => match button {
                0 => self.save(buf, Some(pending)),
                1 => self.proceed(buf, pending),
                _ => {}
            },
            // 0 = Sim, 1 = Não
            Ask::Overwrite {
                mut picker,
                path,
                then,
            } => {
                if button == 0 {
                    match self.try_save(buf, &path) {
                        Ok(()) => {
                            if let Some(pending) = then {
                                self.proceed(buf, pending);
                            }
                        }
                        Err(message) => {
                            picker.set_error(message);
                            self.modal = Modal::Picker { picker, then };
                        }
                    }
                } else {
                    self.modal = Modal::Picker { picker, then };
                }
            }
        }
    }

    // ------------------------------------------------------------------ entrada

    /// Teclado com um diálogo aberto. `ctrl` = Ctrl/Cmd; `shift` = Shift.
    pub(super) fn modal_key(
        &mut self,
        buf: &mut Buffer,
        key: Key,
        ctrl: bool,
        shift: bool,
        layout: &Layout,
    ) {
        // O diálogo sai do estado enquanto é tratado e só volta se continuar aberto.
        match std::mem::replace(&mut self.modal, Modal::None) {
            Modal::None => {}
            Modal::About => {
                if !matches!(key, Key::Enter | Key::Escape | Key::Space | Key::F1) {
                    self.modal = Modal::About;
                }
            }
            Modal::Picker { mut picker, then } => {
                let lay = PickerLayout::new(layout.cols, layout.rows);
                let outcome = picker.key(key, ctrl, lay.items.h);
                self.picker_outcome(buf, picker, then, outcome);
            }
            Modal::Ask { mut dlg, ask } => match dlg.key(key, shift) {
                Some(button) => self.ask_pressed(buf, ask, button),
                None => self.modal = Modal::Ask { dlg, ask },
            },
        }
    }

    /// Texto digitado: só o seletor (campo "Nome:") o aceita.
    pub(super) fn modal_text(&mut self, text: &str) {
        if let Modal::Picker { picker, .. } = &mut self.modal {
            picker.insert_text(text);
        }
    }

    /// Roda do mouse sobre o seletor: rola a lista.
    pub(super) fn picker_wheel(&mut self, lines: isize, layout: &Layout) {
        if let Modal::Picker { picker, .. } = &mut self.modal {
            let visible = PickerLayout::new(layout.cols, layout.rows).items.h;
            picker.scroll_by(lines, visible);
        }
    }

    /// Mouse com um diálogo aberto.
    pub(super) fn modal_pointer(&mut self, buf: &mut Buffer, ptr: Pointer, layout: &Layout) {
        match std::mem::replace(&mut self.modal, Modal::None) {
            Modal::None => {}
            Modal::About => {
                let on_ok = ptr.clicked
                    && ptr
                        .cell
                        .is_some_and(|(cx, cy)| layout.about_ok().contains(cx, cy));
                if !on_ok {
                    self.modal = Modal::About;
                }
            }
            Modal::Ask { dlg, ask } => {
                let lay = dlg.layout(layout.cols, layout.rows);
                let pressed = match ptr.cell {
                    Some((cx, cy)) if ptr.clicked => dlg.hit(&lay, cx, cy),
                    _ => None,
                };
                match pressed {
                    Some(button) => self.ask_pressed(buf, ask, button),
                    None => self.modal = Modal::Ask { dlg, ask },
                }
            }
            Modal::Picker { mut picker, then } => {
                let outcome = self.picker_pointer(&mut picker, ptr, layout);
                self.picker_outcome(buf, picker, then, outcome);
            }
        }
    }

    fn picker_pointer(
        &mut self,
        picker: &mut Picker,
        ptr: Pointer,
        layout: &Layout,
    ) -> PickerOutcome {
        let lay = PickerLayout::new(layout.cols, layout.rows);
        let visible = lay.items.h;

        // Arrastar o polegar da barra de rolagem da lista.
        if ptr.drag_started {
            if let Some((cx, cy)) = ptr.origin {
                if picker.hit(&lay, cx, cy) == PickerHit::Bar(BarHit::Thumb) {
                    let bar = picker.bar(&lay);
                    let grab = bar.track_offset(cx, cy) - bar.thumb().0 as isize;
                    self.drag = Drag::PickerThumb { grab };
                }
            }
        }
        if ptr.dragged {
            if let (Drag::PickerThumb { grab }, Some((cx, cy))) = (self.drag, ptr.cell) {
                let bar = picker.bar(&lay);
                picker.set_scroll(bar.pos_for_thumb(bar.track_offset(cx, cy) - grab), visible);
            }
        }

        match ptr.cell {
            Some((cx, cy)) if ptr.double || ptr.clicked => {
                let hit = picker.hit(&lay, cx, cy);
                picker.click(hit, ptr.double, &lay)
            }
            _ => PickerOutcome::Stay,
        }
    }

    // ------------------------------------------------------------------ desenho

    /// Desenha o diálogo aberto (se houver) por cima de todo o resto.
    pub(super) fn draw_modal(&self, s: &mut Screen, look: &Look, layout: &Layout, caret_on: bool) {
        let (p, g) = (&look.p, &look.g);
        match &self.modal {
            Modal::None => {}
            Modal::About => self.draw_about(s, look, layout),
            Modal::Picker { picker, .. } => {
                let lay = PickerLayout::new(layout.cols, layout.rows);
                picker.draw(s, p, g, &lay, caret_on);
            }
            Modal::Ask { dlg, .. } => {
                let lay = dlg.layout(layout.cols, layout.rows);
                dlg.draw(s, p, g, &lay);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("mini-edit-flow-{}-{}", std::process::id(), name));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn layout() -> Layout {
        Layout::new(106, 34, 10, true)
    }

    fn named(path: PathBuf) -> FileState {
        FileState {
            path: Some(path),
            format: FileFormat::default(),
        }
    }

    #[test]
    fn saving_with_a_path_writes_the_file_and_clears_the_dirty_flag() {
        let dir = scratch("save");
        let path = dir.join("nota.txt");
        let mut ui = AsciiUi::new();
        ui.file = named(path.clone());
        let mut buf = Buffer::new("");
        buf.insert("olá");
        assert!(buf.is_dirty());

        ui.file_cmd(&mut buf, Cmd::Save);
        assert_eq!(fs::read_to_string(&path).unwrap(), "olá");
        assert!(!buf.is_dirty());
        assert!(matches!(ui.modal, Modal::None));
        assert!(ui.message.is_some());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn new_while_dirty_asks_first_and_discard_resets() {
        let mut ui = AsciiUi::new();
        let mut buf = Buffer::new("texto");
        buf.insert("x");
        ui.file_cmd(&mut buf, Cmd::New);
        assert!(matches!(
            ui.modal,
            Modal::Ask {
                ask: Ask::Unsaved(Pending::New),
                ..
            }
        ));

        // a seta para a direita seleciona "Descartar"; Enter confirma
        ui.modal_key(&mut buf, Key::ArrowRight, false, false, &layout());
        ui.modal_key(&mut buf, Key::Enter, false, false, &layout());
        assert!(matches!(ui.modal, Modal::None));
        assert_eq!(buf.text(), "");
        assert!(!buf.is_dirty());
    }

    #[test]
    fn cancelling_keeps_the_document() {
        let mut ui = AsciiUi::new();
        let mut buf = Buffer::new("texto");
        buf.insert("x");
        ui.file_cmd(&mut buf, Cmd::New);
        ui.modal_key(&mut buf, Key::Escape, false, false, &layout());
        assert!(matches!(ui.modal, Modal::None));
        assert_eq!(buf.text(), "xtexto");
        assert!(buf.is_dirty());
    }

    #[test]
    fn clean_documents_skip_the_question() {
        let mut ui = AsciiUi::new();
        let mut buf = Buffer::new("texto");
        ui.file_cmd(&mut buf, Cmd::New);
        assert!(matches!(ui.modal, Modal::None));
        assert_eq!(buf.text(), "");
    }

    #[test]
    fn open_flow_loads_a_file_through_the_picker() {
        let dir = scratch("open");
        fs::write(dir.join("a.txt"), "um\r\ndois\ttab").unwrap();
        let mut ui = AsciiUi::new();
        ui.file = named(dir.join("zzz.txt")); // faz o seletor começar em `dir`
        let mut buf = Buffer::new("");

        ui.file_cmd(&mut buf, Cmd::Open);
        assert!(matches!(ui.modal, Modal::Picker { .. }));

        ui.modal_text("a.txt");
        ui.modal_key(&mut buf, Key::Enter, false, false, &layout());
        assert!(matches!(ui.modal, Modal::None));
        assert_eq!(buf.text(), "um\ndois    tab"); // CRLF -> LF e tab -> 4 espaços
        assert!(!buf.is_dirty());
        assert_eq!(ui.file.path.as_deref(), Some(dir.join("a.txt").as_path()));
        assert!(ui.file.format.crlf);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn open_errors_stay_inside_the_picker() {
        let dir = scratch("openerr");
        fs::write(dir.join("bin.dat"), [0u8, 1, 2]).unwrap();
        let mut ui = AsciiUi::new();
        ui.file = named(dir.join("x"));
        let mut buf = Buffer::new("intacto");

        ui.file_cmd(&mut buf, Cmd::Open);
        ui.modal_text("bin.dat");
        ui.modal_key(&mut buf, Key::Enter, false, false, &layout());
        assert_eq!(buf.text(), "intacto"); // o documento atual não foi tocado
        assert!(matches!(
            ui.modal,
            Modal::Picker { ref picker, .. }
                if picker.error().is_some_and(|e| e.contains("binário"))
        ));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_as_asks_before_overwriting() {
        let dir = scratch("overwrite");
        let target = dir.join("x.txt");
        fs::write(&target, "velho").unwrap();
        let mut ui = AsciiUi::new();
        ui.file = named(dir.join("y.txt"));
        let mut buf = Buffer::new("novo");

        ui.file_cmd(&mut buf, Cmd::SaveAs);
        ui.modal_text("x.txt"); // o campo vinha com "y.txt": digitar substitui
        ui.modal_key(&mut buf, Key::Enter, false, false, &layout());
        assert!(matches!(
            ui.modal,
            Modal::Ask {
                ask: Ask::Overwrite { .. },
                ..
            }
        ));

        // "Não" é o botão padrão: volta ao seletor sem gravar
        ui.modal_key(&mut buf, Key::Enter, false, false, &layout());
        assert!(matches!(ui.modal, Modal::Picker { .. }));
        assert_eq!(fs::read_to_string(&target).unwrap(), "velho");

        // de novo, agora com o atalho "S" (Sim)
        ui.modal_key(&mut buf, Key::Enter, false, false, &layout());
        assert!(matches!(ui.modal, Modal::Ask { .. }));
        ui.modal_key(&mut buf, Key::S, false, false, &layout());
        assert!(matches!(ui.modal, Modal::None));
        assert_eq!(fs::read_to_string(&target).unwrap(), "novo");
        assert_eq!(ui.file.path.as_deref(), Some(target.as_path()));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn quit_with_unsaved_changes_saves_then_quits() {
        let dir = scratch("quit");
        let mut ui = AsciiUi::new();
        ui.file = named(dir.join("q.txt"));
        let mut buf = Buffer::new("");
        buf.insert("dados");

        ui.file_cmd(&mut buf, Cmd::Quit);
        assert!(!ui.want_close);
        assert!(matches!(ui.modal, Modal::Ask { .. }));

        ui.modal_key(&mut buf, Key::Enter, false, false, &layout()); // "Salvar" é o padrão
        assert_eq!(fs::read_to_string(dir.join("q.txt")).unwrap(), "dados");
        assert!(ui.want_close);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn quit_with_discard_does_not_write_anything() {
        let dir = scratch("discard");
        let mut ui = AsciiUi::new();
        ui.file = named(dir.join("q.txt"));
        let mut buf = Buffer::new("");
        buf.insert("dados");

        ui.file_cmd(&mut buf, Cmd::Quit);
        ui.modal_key(&mut buf, Key::D, false, false, &layout()); // atalho de "Descartar"
        assert!(ui.want_close);
        assert!(!dir.join("q.txt").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_untitled_save_opens_the_save_as_dialog() {
        let mut ui = AsciiUi::new();
        let mut buf = Buffer::new("");
        buf.insert("x");
        ui.file_cmd(&mut buf, Cmd::Save);
        assert!(matches!(
            ui.modal,
            Modal::Picker { ref picker, .. } if picker.mode() == PickerMode::SaveAs
        ));
    }

    #[test]
    fn saving_over_a_missing_folder_shows_a_notice() {
        let dir = scratch("badsave");
        let mut ui = AsciiUi::new();
        ui.file = named(dir.join("pasta-inexistente").join("a.txt"));
        let mut buf = Buffer::new("");
        buf.insert("x");
        ui.file_cmd(&mut buf, Cmd::Save);
        assert!(matches!(
            ui.modal,
            Modal::Ask {
                ask: Ask::Notice,
                ..
            }
        ));
        assert!(buf.is_dirty()); // nada foi salvo
        ui.modal_key(&mut buf, Key::Enter, false, false, &layout());
        assert!(matches!(ui.modal, Modal::None));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_file_given_at_startup_is_opened_or_created() {
        let dir = scratch("initial");
        let existing = dir.join("a.txt");
        fs::write(&existing, "oi\n").unwrap();

        let mut ui = AsciiUi::new();
        let mut buf = Buffer::new("antes");
        ui.open_initial(&mut buf, existing.clone());
        assert_eq!(buf.text(), "oi\n");
        assert_eq!(ui.file.path.as_deref(), Some(existing.as_path()));

        let new_file = dir.join("novo.txt");
        let mut ui = AsciiUi::new();
        ui.open_initial(&mut buf, new_file.clone());
        assert_eq!(buf.text(), "");
        assert_eq!(ui.file.path.as_deref(), Some(new_file.as_path()));
        assert!(!new_file.exists()); // só é criado ao salvar
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_about_dialog_only_closes_with_its_keys() {
        let mut ui = AsciiUi::new();
        let mut buf = Buffer::new("");
        ui.modal = Modal::About;
        ui.modal_key(&mut buf, Key::A, false, false, &layout());
        assert!(matches!(ui.modal, Modal::About));
        ui.modal_key(&mut buf, Key::Escape, false, false, &layout());
        assert!(matches!(ui.modal, Modal::None));
    }

    #[test]
    fn clicking_buttons_works_in_the_about_and_question_dialogs() {
        let lay = layout();
        let click = |x: usize, y: usize| Pointer {
            cell: Some((x as isize, y as isize)),
            clicked: true,
            ..Pointer::default()
        };

        let mut ui = AsciiUi::new();
        let mut buf = Buffer::new("");
        ui.modal = Modal::About;
        ui.modal_pointer(&mut buf, click(0, 0), &lay); // fora do botão: continua aberto
        assert!(matches!(ui.modal, Modal::About));
        let ok = lay.about_ok();
        ui.modal_pointer(&mut buf, click(ok.x + 1, ok.y), &lay);
        assert!(matches!(ui.modal, Modal::None));

        // pergunta "alterações não salvas": clicar em "Descartar" (2º botão)
        let mut buf = Buffer::new("a");
        buf.insert("b");
        ui.file_cmd(&mut buf, Cmd::New);
        let discard = match &ui.modal {
            Modal::Ask { dlg, .. } => dlg.layout(lay.cols, lay.rows).buttons[1],
            _ => panic!("era para haver uma pergunta aberta"),
        };
        ui.modal_pointer(&mut buf, click(discard.x + 2, discard.y), &lay);
        assert!(matches!(ui.modal, Modal::None));
        assert_eq!(buf.text(), "");
    }
}
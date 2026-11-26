// Fluxo de arquivos de interface: abrir, salvar, confirmar e avisar

// é um submodulo da Tui.rs para poder usar o estado privado de 'AsciiUi'

use super::{AsciiUi, Drag, Layout, Look, Pointer};
use crate::buffer::Buffer;
use crate::dialogs::{MsgBox, Picker, PickerHit, PickerLayout, PickerMode, PickerOutcome};
use crate::files::{self, FileFormat};
use crate::menu::Cmd;
use crate::screen::Screen;
use crate::scrollbar::BarHit;
use eframe::egui::Key;
use std::path::{Path, PathBuf};


// o que fazer depois de resolver as alterações não salvas
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Pending {
    New,
    Open,
    Quit,
}

// Pergunta e avisos caixas de mensagem
pub(super) enum Ask {
    // Salvar / Descartar / Cancelar antes de executar o Pending
    Unsaved(Pending),
    // Sobreescrever? quarda o seletor para voltar a ele se a resposta for 'nao'
    Overwriter {
        picker: Picker,
        path: PathBuf,
        then: Option<Pending>,
    },
    // Aviso com o botão ok
    Notice,

}

// Diálogo aberto no momento , no maximo um
pub(super) enum Modal {
    None,
    About,
    Picker {
        picker: Picker,
        // O que continuar depois de salvar
        then: Option<Pending>,
    },
    Ask {
        dlg: MsgBox,
        ask: Ask,
    },
}

// Arquivo em edição, de onde veio e para onde vai salvar e como era no disco

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

    // Nome para mostrar no titulo a mensagem
    pub fn display_name(&self) -> String {
        match &self.path {
            Some(path) => files::file_label(path),
            None => "Sem-título".to_string(),
        }
    }

    // Pasta inicial dos seletores
    pub fn dir(&self) -> PathBuf {
        match &self.path {
            Some(path) => files::dir_of(path),
            None => files::start_dir(),
        }
    }

    // Nome sugerido 'Salvar como'
    pub fn suggested_name(&self) -> String {
        match &self.path {
            Some(path) => files::file_label(path),
            None => "sem-titulo.rs".to_string(),
        }
    }
}

impl AsciiUi {
    //---------------------------------------- Comandos de arquivo

    // Novo, Abrir, Salvar, Salvar como e Sair (Os demais são comandos da tui)
    pub(super) fn file_cmd(&mut self, buf: &mut Buffer, cmd: Cmd) {
        match cmd {
            Cmd::New => self.request(buf, Pending::New),
            Cmd::Open => self.request(buf, Pending::Open),
            Cmd::Save => self.save(buf, none),
            Cmd::SaveAs => self.open_picker(PickerMode::SaveAs, None),
            Cmd::Quit => self.request(buf, Pending::Quit),
            _ => {}
        }
    }

    // Executa Pending; Se houver alterações não salvas , pergunta antes
    pub(super) fn request(&mut self, buf: &mut Buffer, pending: Pending){
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
                self.notify("Novo Documento".to_string());
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
        self.modal = Modal::Picker {picker, then};
    }

    // Salvar , sem nome ainda vira "Salvar como" 'then' é o que faz depois
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
            Err(message) => self.show_notice("erro ao salvar" + &message),
        }
    }

    fn try_save(&mut self, buf: &mut Buffer, path: &Path) -> Result<(), String> {
        files::save(path, &buf.text(), self.file.format)?;
        buf.mark_saved();
        self.file.path = Some(path.to_path_buf());
        self.notify(format!("Salvo: {}", files::file_label(path)));
        Ok(())
    }

    // Lê o path para dentro do editor. Se falhar , o documento atual fica intacto
    fn load_into(&mut self, buf: &mut Buffer, path: PathBuf) -> Result<(), String> {

        let loaded = files::load(&path)?;
        let label = files::file_label(&path);
        let file = FileState {

            path: Some(path),
            format: loaded.format,
        };
        self.replace_document(buf, &loaded.text, file);
        let mut message = format!("Alberto: {label}");
        if loaded.tabs {
            message.push_str(" (tabs convertidos em 4 espaços)");
        }
        self.notify(message);
        Ok(())
    }

    // Abre o path ao iniciar (argumento de linha de comando). Se o arquivo não existe
    // começa com um documento vazio com esse nome, como 'nano novo.txt'

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

    // -------------------------- respostas
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

    // A pessoa escolheu `path` no seletor. Erros ficam dentro do seletor, para ela
    // poder tentar outro arquivo sem recomeçar.

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
                    self.modal = Modal::Picker { picker, then};
                }
            }
            PickerMode::SaveAs => {
                let same_file = self.file.path.as_deref() == Some(path.as_path());
                if path.exists() && !same_file {
                    let dlg = MsgBox::overwriter(&files::file_label(&path));
                    self.modal = Modal::Ask {
                        dlg,
                        ask: Ask::Overwriter {picker, path, then},
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
                            self.modal = Modal::Picker {picker, then};
                        }
                    }
                }
            }
        }
    }

    // Um botão de uma caixa de mensagem foi adicionado

    fn ask_pressed(&mut self, buf: &mut Buffer, ask: Ask, button: usize) {
        match ask {
            Ask::Notice => {}
            // 0 = salvar , 1 = descartar , 2 = cancelar
            Ask::Unsaved(pending) => match button {
                0 => self.save(buf, Some(pending)),
                1 => self.proceed(buf,  pending),
                _ => {}
            },
            // 0 = Sim , 1 = nao
            Ask::Overwriter {
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
                            self.modal = Modal::Picker { picker, then};
                        }
                    }
                } else {
                    self.modal = Modal::Picker {picker, then};
                }
            }
        }
    }

    // ----------------------------------------- Entrada

    // Teclado com um diálogo aberto 'ctrl' = ctrl/Cmd 'shift' = shift
    pub(super) fn modal_key(
        &mut self, 
        buf: &mut Buffer,
        key: Key,
        ctrl: bool,
        shift: bool,
        layout: &Layout,
    ) {
        // O dialogo sai do estado enquanto é tratado e só volta se continuar aberto
        match std::mem::replace(&mut self.modal, Modal::None) {
            Modal::None => {}
            Modal::About => {
                if !matches!(key, Key::Enter | Key::Escape | Key::Space | Key::F1) {
                    self.modal = Modal::About;
                }
            }
            Modal::Picker {mut picker, then} => {
                let lay = PickerLayout::new(layout.cols, layout.rows);
                let outcome = picker.key(key, ctrl, lay.items.h);
                self.picker_outcome(buf, picker, then, outcome);
            }
            Modal::Ask {mut dlg, ask} => match dlg.key(key, shift){
                Some(button) => self.ask_pressed(buf, ask, button),
                None => self.modal = Modal::Ask {dlg, ask},
            },
        }
    }

    // Texto digitado, só o seletor "Campo nome" o aceita
    pub(super) fn modal_text(&mut self, text: &str) {
        if let Modal::Picker {picker,..} = &mut self.modal{
            picker.insert_text(text);
        }
    }

    // Roda do mouse sobre o seletor: Rola a lista
    pub(super) fn picker_wheel(&mut self, lines: isize, layout: &Layout) {
        if let Modal::Picker { picker, ..} =  &mut self.modal {
            let visible = PickerLayout::new(layout.cols, layout.rows).items.h;
            picker.scroll_by(lines, visible);
        }
    }

    // Mouse com o dialogo aberto
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

    // Parei na função fn picker_pointer() na linha 388 do
    fn picker_pointer(

        &mut self,
        picker: &mut Picker,
        ptr: Pointer,
        layout: &Layout,

    ) -> PickerOutcome {
        let lay = PickerLayout::new(layout.cols, layout.rows);
        let visible = lay.items.h;

        // Arrastar o polegar da barra de rolagem da lista
        if ptr.drag_starterd {
            if let Some((cx, cy)) = ptr.origin {
                if picker.hit(&lay, cx, cy) == PickerHit::Bar(BarHit::Thumb) {
                    let bar = picker.bar(&lay);
                    let grab = bar.track_offset(cx, cy) - bar.thumb().0 as isize;
                    self.drag = Drag::PickeThumb {grab};
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
            Some ((cx, cy)) if ptr.double || ptr.clicked => {
                let hit = picker.hit(&lay, cx, cy);
                picker.click(hit, ptr.double, &lay)
            }
            _ => PickerOutcome::Stay,
        }
    }

    // ------------------------------------------ Desenho
    // Desenha o dialogo aberto (se houver) por cima de todo o resto

    pub(super) fn draw_modal(&self, s: &mut Screen, look: &Look, layout: &Layout, caret_on: bool) {
        let (p, g) = (&look.p, &look.g);
        
        match &self.modal {
            Modal::None => {}
            Modal::About => self.draw_about(s, look, layout),
            Modal::Picker { picker, ..} => {
                let lay = PickerLayout::new(layout.cols, layout.rows);
                picker.draw(s, p, g, &lay, caret_on);
            }
            Modal::Ask {dlg, ..} => {
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
            std::env::temp_dir().join(format!("Beaver-Ide-{}-{}", std::process::id(), name));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrape();
        dir
    }

    fn layout() -> Layout {
        Layout::new(106,34, 10, true)
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
        let mut buff = Buffer::new("texto");
        buf.insert("x");
        ui.file_cmd(&mut buf, Cmd::New);
        assert!(matches!(
            ui.modal,
            Modal::Ask {
                ask: Ask::Unsaved(Pending::New),
                ..
            }
        ));

        // A seta para a direita seleciona "Descartar": Enter Confirmar
        ui.modal_key(&mut buf, Key::ArrowRight, false, false, &layout());
        ui.modal_key(&mut buf, Key::Enter, false, false, &layout());
        assert!(matches!(ui.modal, Modal::None));
        assert_eq!(buf.text(), "");
        assert!(!buf.is_dirty());
    }

    // Parei aqui, correspondente a linha 507 do claude
    #[test]
    fn cancelling_keeps_the_document() {

    }
}
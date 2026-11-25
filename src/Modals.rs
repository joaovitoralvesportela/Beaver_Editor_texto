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
    

}
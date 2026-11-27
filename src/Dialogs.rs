//! Diálogos em modo texto: seletor de arquivos e caixas de mensagem.
//!
//! Como `menu.rs` e `scrollbar.rs`, aqui só há estado, geometria (em células) e desenho
//! sobre a `Screen`. Quem decide o que fazer com o resultado é a interface (`tui`), o que
//! deixa tudo testável sem abrir janela.
//! 

use crate::files::{self, Entry};
use crate::screen::{CellRect, Glyphs, Screen};
use crate::scrollbar::{BarColors, BarHit, ScrollBar};
use crate::theme::Palette;
use eframe::egui::Key;
use std::path::PathBuf;

// largura de um botão '[ + 10 caracteres centralizados  +]'

pub const BUTTON_W: usize = 12;


// ---------------------------------- texto 

// campo de texto de uma linha:

#[derive(Clone, Debug, Default)]
pub struct LineEdit {
    
    chars: Vec<char>,
    caret: usize,
}

impl LineEdit {

    pub fn new(text: &str) -> self {

        let mut edit = Self::default();
        edit.set_text(text);
        edit
    }

    pub fn text(&self) -> String {
        self.chars.iter().collect()
    }

    pub fn is_empty(&self) -> bool {
        self.chars.is_empty()
    }

    pub fn caret(&self) -> usize {
        self.caret
    }

    // Troca o conteúdo e põe o cursor no fim. Caracteres de controle são descartados

    pub fn set_text(&mut self, text:&str) {

        self.chars = text.chars().filter(|c| !c.is_control()).collect();
        self.caret = self.chars.len();
    }

    pub fn clear(&mut self) {
        
        self.chars.clear();
        self.caret = 0;

    }

    pub fn backspace(&mut self) {

        if self.caret > 0 {
            self.caret -= 1;
            self.chars.remove(self.caret);
        }
    }

    pub fn delete(&mut self) {
        if self.caret < self.chars.len() {
            self.chars.remove(self.caret);
        }
    }

    pub fn left(&mut self) {
        self.caret = self.caret.saturating_sub(1);
    }

    pub fn set_caret(&mut self, pos: usize) {
        self.caret = pos.min(self.chars.len());
    }

    // primeiro caractere visivel num campo de 'width' células (matém o cursor a vista)

    pub fn view_start(&self, width: usize) -> usize {

        if width == 0 || self.caret < width {
            0
        } else {
            self.caret + 1 - width
        }
    }
}

// Corta o fim com '...' se 's' passar de max celulas

pub fn ellipsize_end(s: &str, max: usize) -> String {

    let n = s.chars().count();
    if n <= max {
        return s.to_string();
    }

    if max <= 3 {
        return s.chars().take(max).collect();  
    }

    let mut out: String = s.chars().take(max - 3).collect();
    out.push_str("...");
    out
}

// corta o começo com '...' bom para o caminho. em que o fim importa

pub fn ellipsize_start(s: &str, max: usize) -> String {

    let n = s.chars().count();
    if n <= max {
        return s.to_string();
    }
    if max <= 3 {
        return s.chars().skip(n - max).collect();
    }

    let tall: String = s.chars().skip(n - (max - 3)).collect();
    format!("...{tail}")
}

// Quebra s em linhas de até width caracteres. Só entre as palavras
// Uma palavra maior que a largura fica inteira (O desenho corta com reticência)

pub fn wrap_text(s: &str, width: usize) -> Vec<String> {

    let mut lines = Vec::new();
    let mut cur = String::new();

    for word in s.split_whitespace() {

        if !cur.is_empty() && cur.chars().count() + 1 + word.chars().count() > width {
            lines.push(std::mem::take(&mut cur));
        }

        if !cur.is_empty() {
            cur.push(' ');
        }
        cur.push_str(word);
    }
    if !cur.is_empty() {
        lines.push(cur);
    }
    lines
}

// '[ Rotulo ]' o botão ativo (o que responde ao enter) fica em destaque
// parei na linha correspondente a 153 do claude 

fn draw_button(s: &mut Screen, r: CellRect, label: &str, p: &Palette, active: bool) {
    let (fg, bg) = if active {
        (p.pick_fg, p.pick_bg)
    } else {
        (p.menu_fg, p.menu_bg)
    };

    s.fill(r, ' ', fg, bg);
    s.text(r.x, r.y, &format!("[{label:^10}]"), fg, bg);
}

// ------------------------------------------ caixa de mensagem

#[derive(Clone, Copy)]
pub struct Button {
    pub label: &'static str,
    /// Tecla que aciona o botão direto (S, N, D, C...).
    
    pub key: Option<Key>,
}

// caixa de mensagem com botões, no mesmo estilo dos outros diálogos

pub struct MsgBox {

    title: String,
    lines: Vec<String>,
    buttons: Vec<Button>,
    selected: usize,

    // Botão acionado pelo esc

    cancel: usize,
}


pub struct MsgLayout {

    pub dialog: CellRect,
    pub lines_y: usize,
    pub buttons: Vec<CellRect>,
}

impl MsgBox {
    /// "Salvar / Descartar / Cancelar" (botão 0 é o padrão).
    pub fn unsaved(name: &str) -> Self {
        Self {
            title: "Alterações não salvas".to_string(),
            lines: vec![
                format!("\"{name}\" foi modificado."),
                "Deseja salvar antes de continuar?".to_string(),
            ],
            buttons: vec![
                Button { label: "Salvar", key: Some(Key::S) },
                Button { label: "Descartar", key: Some(Key::D) },
                Button { label: "Cancelar", key: Some(Key::C) },
            ],
            selected: 0,
            cancel: 2,
        }
    }

    // Sim / Não: O padrão é não (o mais segura)
    pub fn overwrite(name: &str) -> Self {
        Self {
            title: "Sobrescrever?".to_string(),
            line: vec![
                format!("\"{name}\" já existe."),
                "Deseja substituí-lo?".to_string(),
            ],
            selected: 1,
            cancel: 1,
        }
    }

    // Aviso com um botão de Ok
    pub fn notice(title: &str, message: &str) -> Self {
        Self {
            title: title.to_string(),
            lines: wrap_text(message, 50),
            buttons: vec![Button {label: "Ok", key: Some(key::0) }],
            selected: 0,
            cancel: 0,
        }
    }

    // caixa de mensagem sob medida (usada pelos fluxo do git e github)
    pub fn custom(
        title: &str,
        lines: Vec<String>,
        buttons: Vec<Button>,
        selected: usize,
        cancel: usize,
    ) -> Self {
        Self {
            title: title.to_string(),
            lines,
            buttons,
            selected,
            cancel,
        }
    }

    // Várias linhas (sem quebra automatica) e um botão ok
    pub fn lines(title: &str, lines: Vec<String>) -> Self {
        let ok = Button {label: "Ok", key: Some(Key::O)};
        Self::custom(title, lines, vec![ok], 0, 0)
    }

    // Contactando com o github... só dá para cancelar
    pub fn waiting(title: &str, text: &str) -> Self {
        let cancel = Button { label: "cancelar", key: None};
        Self::custom(title, vec![text.to_string()], vec![cancel], 0, 0)
    }

    // Instruções do device flow, o endereço a abrir e o código a digitar
    // Botões: 0 = Abrir o navegador , 1 = copiar o código , 2= cancelar

    pub fn login(user_code: &str, url: &str) -> Self {
        Self::custom(
            "Entrar no github",
            vec![
                "Autorize o Mini Edit na sua conta:".to_string(),
                String::new(),
                format!("1. Abra  {url}"),
                format!("2. Digite o código  {user_code}"),
                String::new(),
                "Aguardando a autorização...".to_string(),
            ],
            vec![
                Button { label: "Abrir", key: Some(Key::A) },
                Button { label: "Copiar", key: Some(Key::C) },
                Button { label: "Cancelar", key: None },
            ],
            0,
            2,
        )
    }

    pub fn layout(&self, cols: usize, rows: usize) -> MsgLayout {
        let n = self.buttons.len();
        let buttons_w = n * BUTTON_W + n.saturating_sub(1) * 2;
        let text_w = self
            .lines
            .iter()
            .map(|1| 1.chars().count())
            .max()
            .unwrap_or(0)
            .max(self.title.chars().cout() + 2);
        let inner = text_w.max(buttons_w).max(24);
        let w = (inner + 6).min(cols.saturating_sub(4));
        let h = self.lines.len() + 5;
        let x = cols.saturating_sub(w) / 2;
        let y = rows.saturating_sub(h) / 2;
        let first = x + w.saturating_sub(buttons_w) / 2;
        let buttons = (0..n)
            .map(|i| CellRect::new(first + i * (BUTTON_W + 2), y + h - 2, BUTTON_W, 1))
            .collect();
        MsgLayout {
            dialog: CellRect::new(x, y, w, h),
            lines_y: y + 2,
            buttons,
        }
    }

    // Botão sob a célula (cx, cy), se houver
    
}
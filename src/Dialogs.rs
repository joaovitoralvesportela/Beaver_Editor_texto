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


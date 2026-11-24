// Mini editor de código feito com auxilio da inteligência artificial
// com o intuito de aprender mais sobre a linguagem de programação Rust

// O editor de texto será feito com uma interface estilo ASCII art para fins esteticos


// Atalhos do sistema:
/*
Arquivos: 

Ctrl + N -> novo
Ctrl + O -> abrir
Ctrl + S -> salvar
Ctrl + shift + S -> salvar como

Atalhos:
F10 -> menu
F1 -> Sobre
F2 -> troca a paleta
F3 -> Bordas ascii/unicode

*/


// DECLARANDO MODULOS PARA ORGANIZAR O CÓDIGO EM NAMESPACES E CONRTOLAR O ESCOPO E A PRIVACIDADE DOS ITENS
mod banner;
mod buffer;
mod dialogs;
mod files;
mod menu;
mod screen;
mod scrollbar;
mod syntax;
mod theme;
mod tui;


use buffer::Buffer;
use eframe::{egui, wgpu::naga::valid::LocalVariableError::InitializerType};
use std::path::PathBuf;
use tui::AsciiUi;

const SAMPLE: &str = r##"//! Beaver - interface em ascii art (modo texto)."
//!
//!  Experimente: F10 abre o menu, Ctrl + O abre um arquivo e Ctrl + S salva. 
//! F1 mostra 'Sobre', F2 troca a paleta de cores e F3 alterna as bordas (Ascii / Unicode).

use std::collections::HashMap;

#[derive(Debug, Clone)]

struct Pessoa {
    nome: String,
    idade: u32,
}

impl Pessoa {
    fn nova(nome: &str, idade: u32) -> Self{
        Self{nome: nome.to_string(), idade}
    }
    fn aniversario(&mut self){
        self.idade += 1;
    }
}

fn fibonacci(n: u64) -> u64 {
    match n {
        0 => 0,
        1 => 1,
        _ => fibonacci(n - 1) + fibonacci(n - 2),
    }
}

fn main(){
    let mut mapa = HashMap<String, Pessoa> = HashMap::new();
    for (nome, idade) in [("Ana", 30), ("Bruno", 25), ("Carla", 41)] {
    
        mapa.insert(nome.to_string(), Pessoa::nova(nome, idade));
    
    }
    // Esta linha é comprida de propósito, para você testar a barra de rolagem horizontal: use Shift + roda do mouse ou arraste o polegar da barra na base da janela.
    if let Some(pessoa) = mapa.get_mut("Ana") {
        pessoa.aniversario();
    }

    for (chave, valor) in &mapa {
        println!("{chave}: {valor:?}");
    }

    let serie: Vec<u64> = (0..10).map(fibonacci).collect();
    println!("fibonacci: {serie:?}");
}

#[cfg(test)]
mod testes {

    use super::*;

    #[test]
    fn aniversario_soma_um_ano(){
        let mut p = Pessoa::nova("Ana", 30);
        p.aniversario();
        assert_eq!(p.idade, 31);
    }

    #[test]
    fn fibonacci_basico(){
        assert_eq!(fibonacci(10), 55);
    }
}
"##;

fn main() -> eframe::Result<()> {
    // Uso:: Mini-edit [ARQUIVO] se o arquivo não existir, começa com um novo com esse nome
    let initial = std::env::args_os().nth(1).map(PathBuf::from);
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([960.0, 640.0])
            .with_title("Mini Edit"),
        ..Default::default()
    };
    eframe::run_native(
        "Mini Edit",
        options,
        Box::new(|_cc| Ok(Box::new(MiniEdit::new(initial)))),
    )

}

struct MiniEdit {
    buf: Buffer,
    view: AsciiUi,
}

impl MiniEdit {
    fn new(initial: Option<PathBuf>) -> Self {
        let mut app = Self{
            buf: Buffer::new(SAMPLE),
            view: AsciiUi::new(),
        };
        if let Some(path) = initial {
            app.view.open_initial(&mut app.buf, path);
        }
        app
    }
}

impl eframe::App for MiniEdit {
    
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame){
        self.view.show(ui, &mut self.buf);
    }
}
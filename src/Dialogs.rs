//! Diálogos em modo texto: seletor de arquivos e caixas de mensagem.
//!
//! Como `menu.rs` e `scrollbar.rs`, aqui só há estado, geometria (em células) e desenho
//! sobre a `Screen`. Quem decide o que fazer com o resultado é a interface (`tui`), o que
//! deixa tudo testável sem abrir janela.
//! 


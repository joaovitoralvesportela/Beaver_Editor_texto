//! Interface em "ASCII art": monta a tela de modo texto a partir do estado do editor e
//! traduz mouse e teclado em comandos sobre o `Buffer`.
//!
//! Fluxo de cada frame (o mesmo de uma TUI em modo imediato):
//!   1. mede a fonte e descobre quantas colunas x linhas de células cabem na janela;
//!   2. calcula o `Layout` (onde ficam menu, janela, barras e status);
//!   3. trata mouse e teclado, alterando o modelo e o estado da interface;
//!   4. recalcula o layout (o texto pode ter mudado) e compõe a grade de células;
//!   5. converte a grade em formas do egui (`Screen::paint`).

mod modals;

use crate::banner;
use crate::buffer::Buffer;
use crate::dialogs::ellipsize_end;
use crate::menu::{self, Cmd, MENUS};
use crate::screen::{CellRect, Glyphs, Screen, ASCII, UNICODE};
use crate::scrollbar::{BarColors, BarHit, ScrollBar};
use crate::syntax::{tokenize, Token};
use crate::theme::{Palette, PALETTES};
use eframe::egui::{self, Color32, FontId, Pos2, Sense};
use modals::{FileState, Modal, Pending};

const MIN_COLS: usize = 64;
const MIN_ROWS: usize = 20;
/// Largura reservada ao indicador "[ linha:coluna ]" na borda inferior da janela.
const POS_LABEL_W: usize = 13;
const ABOUT_W: usize = 56;
const ABOUT_H: usize = 16;
const BUTTON: &str = "[  OK  ]";
const BUTTON_W: usize = BUTTON.len();

#[derive(Clone, Copy, PartialEq)]
enum MenuState {
    Closed,
    Open { menu: usize, item: usize },
}

#[derive(Clone, Copy, PartialEq)]
enum Drag {
    Idle,
    Select,
    VThumb { grab: isize },
    HThumb { grab: isize },
    /// Arrastando o polegar da barra de rolagem da lista do seletor de arquivos.
    PickerThumb { grab: isize },
}

/// Paleta + conjunto de caracteres em uso neste frame.
#[derive(Clone, Copy)]
struct Look {
    p: Palette,
    g: Glyphs,
}

/// Conversão pixel <-> célula.
#[derive(Clone, Copy)]
struct Grid {
    origin: Pos2,
    cw: f32,
    rh: f32,
}

impl Grid {
    /// Célula sob o ponto `p`. Pode ser negativa ou passar da grade (mouse fora da janela).
    fn cell_at(&self, p: Pos2) -> (isize, isize) {
        (
            ((p.x - self.origin.x) / self.cw).floor() as isize,
            ((p.y - self.origin.y) / self.rh).floor() as isize,
        )
    }
}

/// O que o mouse fez neste frame, já em células (usado pelos diálogos).
#[derive(Clone, Copy, Default)]
struct Pointer {
    /// Célula sob o ponteiro.
    cell: Option<(isize, isize)>,
    /// Célula onde um arrasto começou.
    origin: Option<(isize, isize)>,
    /// Clique simples (não conta o clique que faz parte de um duplo ou triplo).
    clicked: bool,
    double: bool,
    drag_started: bool,
    dragged: bool,
}

/// Onde fica cada parte da interface, em células. É função pura de (tamanho, estado),
/// então o desenho e o hit-test do mouse sempre concordam.
#[derive(Clone, Copy)]
struct Layout {
    cols: usize,
    rows: usize,
    /// Moldura da janela de edição (a linha 0 é a barra de menu; a última, a de status).
    window: CellRect,
    gutter: CellRect,
    text: CellRect,
    vbar: CellRect,
    hbar: CellRect,
    about: CellRect,
}

impl Layout {
    /// Exige `cols >= MIN_COLS` e `rows >= MIN_ROWS` (quem chama garante).
    fn new(cols: usize, rows: usize, n_lines: usize, show_numbers: bool) -> Self {
        let digits = n_lines.to_string().len().max(2);
        let gutter_w = if show_numbers { digits + 2 } else { 0 };
        let inner_w = cols - 2; // dentro da moldura (a barra vertical fica NA moldura)
        let inner_h = rows - 4;
        let gutter = CellRect::new(1, 2, gutter_w.min(inner_w), inner_h);
        let text = CellRect::new(1 + gutter.w, 2, inner_w - gutter.w, inner_h);

        // A barra horizontal também fica na moldura: à direita do indicador de posição.
        let hx = 2 + POS_LABEL_W + 1;
        let hbar = if cols > hx + 6 {
            CellRect::new(hx, rows - 2, cols - 1 - hx, 1)
        } else {
            CellRect::new(0, 0, 0, 0)
        };

        let about_w = ABOUT_W.min(cols - 2);
        Self {
            cols,
            rows,
            window: CellRect::new(0, 1, cols, rows - 2),
            gutter,
            text,
            vbar: CellRect::new(cols - 1, 2, 1, inner_h),
            hbar,
            about: CellRect::new((cols - about_w) / 2, (rows - ABOUT_H) / 2, about_w, ABOUT_H),
        }
    }

    fn about_ok(&self) -> CellRect {
        let r = self.about;
        CellRect::new(r.x + r.w.saturating_sub(BUTTON_W) / 2, r.y + 14, BUTTON_W, 1)
    }
}

pub struct AsciiUi {
    screen: Screen,
    font: FontId,
    palette: usize,
    unicode: bool,
    line_numbers: bool,
    /// Primeira linha / coluna visíveis (a rolagem é em unidades de célula).
    scroll_y: usize,
    scroll_x: usize,
    /// Fração de célula acumulada pela roda do mouse / touchpad.
    wheel: (f32, f32),
    menu: MenuState,
    /// Diálogo aberto (sobre, seletor de arquivos, perguntas).
    modal: Modal,
    drag: Drag,
    /// Pedido para rolar até o cursor depois de digitar ou mover.
    reveal: bool,
    /// Instante da última ação do usuário (reinicia o piscar do cursor).
    last_activity: f64,
    needs_focus: bool,
    hover: Option<(isize, isize)>,
    /// Arquivo em edição (caminho e formato original).
    file: FileState,
    /// Aviso temporário da barra de status e o instante em que expira.
    message: Option<(String, f64)>,
    /// Relógio do frame atual (os avisos dos fluxos de arquivo precisam dele).
    now: f64,
    /// A janela pode fechar: as alterações não salvas já foram resolvidas.
    allow_close: bool,
    /// Pedido de fechar a janela; o `show` o aplica no fim do frame.
    want_close: bool,
    /// Último título enviado ao sistema operacional.
    os_title: String,
}

impl AsciiUi {
    pub fn new() -> Self {
        Self {
            screen: Screen::default(),
            font: FontId::monospace(15.0),
            palette: 0,
            unicode: false, // ASCII puro por padrão: funciona em qualquer fonte
            line_numbers: true,
            scroll_y: 0,
            scroll_x: 0,
            wheel: (0.0, 0.0),
            menu: MenuState::Closed,
            modal: Modal::None,
            drag: Drag::Idle,
            reveal: false,
            last_activity: 0.0,
            needs_focus: true,
            hover: None,
            file: FileState::untitled(),
            message: None,
            now: 0.0,
            allow_close: false,
            want_close: false,
            os_title: String::new(),
        }
    }

    /// Um frame completo da interface. Ocupa a janela inteira.
    pub fn show(&mut self, ui: &mut egui::Ui, buf: &mut Buffer) {
        let size = ui.available_size();
        let (rect, response) = ui.allocate_exact_size(size, Sense::click_and_drag());
        let painter = ui.painter().clone();
        let look = Look {
            p: PALETTES[self.palette],
            g: if self.unicode { UNICODE } else { ASCII },
        };
        let now = ui.input(|i| i.time);
        self.now = now;

        // Fechar a janela (X, Alt+F4) com alterações não salvas: pergunta antes.
        if ui.input(|i| i.viewport().close_requested()) && !self.allow_close && buf.is_dirty() {
            ui.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            if matches!(self.modal, Modal::None) {
                self.request(buf, Pending::Quit);
            }
        }

        // 1) Medidas da célula: fonte monoespaçada => toda célula tem o mesmo tamanho.
        //    A altura é arredondada ao pixel físico para as linhas não "tremerem".
        let probe =
            painter.layout_no_wrap("MMMMMMMMMM".to_owned(), self.font.clone(), Color32::WHITE);
        let ppp = ui.pixels_per_point();
        let cw = probe.size().x / 10.0;
        let rh = ((probe.size().y * ppp).round() / ppp).max(1.0);
        let cols = (rect.width() / cw).floor() as usize;
        let rows = (rect.height() / rh).floor() as usize;
        let grid = Grid { origin: rect.min, cw, rh };

        painter.rect_filled(rect, egui::CornerRadius::default(), look.p.desktop);
        if self.needs_focus {
            response.request_focus();
            self.needs_focus = false;
            ui.request_repaint();
        }

        if cols < MIN_COLS || rows < MIN_ROWS {
            let mut screen = std::mem::take(&mut self.screen);
            screen.reset(cols, rows, look.p.text_fg, look.p.desktop);
            screen.text(0, 0, "Aumente a janela", look.p.text_fg, look.p.desktop);
            screen.paint(&painter, grid.origin, cw, rh, &self.font, look.p.desktop);
            self.screen = screen;
            return;
        }

        // 2) Layout e 3) entrada do usuário.
        let layout = Layout::new(cols, rows, buf.line_count(), self.line_numbers);
        self.on_pointer(ui, buf, &response, &grid, &layout, now);

        if response.has_focus() {
            // Sem isto o egui usaria Tab, setas e Esc para mexer no foco entre widgets.
            ui.memory_mut(|mem| {
                mem.set_focus_lock_filter(
                    response.id,
                    egui::EventFilter {
                        tab: true,
                        horizontal_arrows: true,
                        vertical_arrows: true,
                        escape: true,
                        ..Default::default()
                    },
                );
            });
            let events = ui.input(|i| i.events.clone());
            for ev in &events {
                self.on_event(ui, buf, ev, &layout, &grid, now);
            }
        }

        // 4) O texto pode ter mudado (nº de linhas => largura do gutter): refaz o layout.
        let layout = Layout::new(cols, rows, buf.line_count(), self.line_numbers);
        if std::mem::take(&mut self.reveal) {
            self.reveal_caret(buf, &layout);
        }
        self.clamp_scroll(buf, &layout);

        let focused = response.has_focus();
        let mut screen = std::mem::take(&mut self.screen);
        self.compose(&mut screen, buf, &layout, &look, focused, now);

        // 5) Grade de células -> formas do egui.
        screen.paint(&painter, grid.origin, cw, rh, &self.font, look.p.desktop);
        self.screen = screen;

        // Título da janela do sistema: nome do arquivo e `*` se foi modificado.
        let title = format!(
            "{}{} - Mini Edit",
            self.file.display_name(),
            if buf.is_dirty() { "*" } else { "" }
        );
        if title != self.os_title {
            ui.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.os_title = title;
        }
        // Os fluxos de arquivo pedem para fechar (Sair) com esta flag.
        if std::mem::take(&mut self.want_close) {
            self.allow_close = true;
            ui.send_viewport_cmd(egui::ViewportCommand::Close);
        }

        if focused {
            // O egui só repinta quando há input; o cursor piscando precisa disto.
            ui.request_repaint_after(std::time::Duration::from_millis(250));
        }
    }

    // ------------------------------------------------------------------ estado

    /// Há um menu ou diálogo capturando a entrada?
    fn captured(&self) -> bool {
        !matches!(self.modal, Modal::None) || matches!(self.menu, MenuState::Open { .. })
    }

    /// Qualquer ação do usuário: reinicia o piscar do cursor e pede para mostrá-lo.
    fn touch(&mut self, now: f64) {
        self.last_activity = now;
        self.reveal = true;
    }

    fn bars(&self, buf: &Buffer, layout: &Layout) -> (ScrollBar, ScrollBar) {
        let v = ScrollBar {
            rect: layout.vbar,
            vertical: true,
            content: buf.line_count(),
            view: layout.text.h,
            pos: self.scroll_y,
        };
        let h = ScrollBar {
            rect: layout.hbar,
            vertical: false,
            content: buf.max_cols() + 1, // +1: o cursor pode ficar depois do último caractere
            view: layout.text.w,
            pos: self.scroll_x,
        };
        (v, h)
    }

    fn clamp_scroll(&mut self, buf: &Buffer, layout: &Layout) {
        let max_y = buf.line_count().saturating_sub(layout.text.h);
        let max_x = (buf.max_cols() + 1).saturating_sub(layout.text.w);
        self.scroll_y = self.scroll_y.min(max_y);
        self.scroll_x = self.scroll_x.min(max_x);
    }

    /// Rola o mínimo necessário para o cursor ficar visível.
    fn reveal_caret(&mut self, buf: &Buffer, layout: &Layout) {
        let (line, col) = buf.line_col(buf.sel.head);
        let (h, w) = (layout.text.h, layout.text.w);
        if line < self.scroll_y {
            self.scroll_y = line;
        } else if line >= self.scroll_y + h {
            self.scroll_y = line + 1 - h;
        }
        if col < self.scroll_x {
            self.scroll_x = col;
        } else if col >= self.scroll_x + w {
            self.scroll_x = col + 1 - w;
        }
    }

    /// Célula da tela -> índice de caractere no texto (com *clamp* aos limites).
    fn pos_from_cell(&self, buf: &Buffer, layout: &Layout, cx: isize, cy: isize) -> usize {
        let last_line = buf.line_count() as isize - 1;
        let line = (self.scroll_y as isize + cy - layout.text.y as isize).clamp(0, last_line);
        let col = (self.scroll_x as isize + cx - layout.text.x as isize).max(0);
        buf.pos_at(line as usize, col as usize)
    }

    // ------------------------------------------------------------------- mouse

    fn on_pointer(
        &mut self,
        ui: &egui::Ui,
        buf: &mut Buffer,
        resp: &egui::Response,
        grid: &Grid,
        layout: &Layout,
        now: f64,
    ) {
        self.hover = ui
            .input(|i| i.pointer.hover_pos())
            .map(|p| grid.cell_at(p));
        let cell = resp.interact_pointer_pos().map(|p| grid.cell_at(p));
        let shift = ui.input(|i| i.modifiers.shift);

        if resp.clicked() || resp.drag_started() || resp.double_clicked() || resp.triple_clicked()
        {
            resp.request_focus();
        }
        if resp.drag_stopped() {
            self.drag = Drag::Idle;
        }

        // Diálogo aberto: é modal, só ele responde ao mouse.
        if !matches!(self.modal, Modal::None) {
            let double = resp.double_clicked();
            let ptr = Pointer {
                cell,
                origin: if resp.drag_started() {
                    ui.input(|i| i.pointer.press_origin())
                        .map(|p| grid.cell_at(p))
                } else {
                    None
                },
                clicked: resp.clicked() && !double && !resp.triple_clicked(),
                double,
                drag_started: resp.drag_started(),
                dragged: resp.dragged(),
            };
            self.modal_pointer(buf, ptr, layout);
            return;
        }

        // Menu aberto: passar o mouse seleciona; clicar executa, troca de menu ou fecha.
        if let MenuState::Open { menu: open, .. } = self.menu {
            if let Some((hx, hy)) = self.hover {
                if let Some(i) = menu::item_at(open, layout.cols, hx, hy) {
                    self.menu = MenuState::Open { menu: open, item: i };
                }
            }
            if resp.clicked() {
                match cell {
                    Some((cx, cy)) => self.click_open_menu(ui, buf, layout, open, cx, cy),
                    None => self.menu = MenuState::Closed,
                }
            }
            return;
        }

        // Arrastos: barras de rolagem e seleção de texto.
        if resp.drag_started() {
            let origin = ui
                .input(|i| i.pointer.press_origin())
                .map(|p| grid.cell_at(p));
            if let Some((cx, cy)) = origin {
                self.begin_drag(buf, layout, cx, cy, shift, now);
            }
        }
        if resp.dragged() {
            if let Some((cx, cy)) = cell {
                self.update_drag(buf, layout, cx, cy, now);
            }
            ui.request_repaint(); // mantém o autoscroll andando com o mouse parado
        }

        // Cliques.
        if let Some((cx, cy)) = cell {
            if resp.double_clicked() {
                if layout.text.contains(cx, cy) {
                    let pos = self.pos_from_cell(buf, layout, cx, cy);
                    let word = buf.word_range_at(pos);
                    buf.set_selection(word.start, word.end);
                    self.last_activity = now;
                }
            } else if resp.triple_clicked() {
                if layout.text.contains(cx, cy) {
                    let pos = self.pos_from_cell(buf, layout, cx, cy);
                    let line = buf.line_range_at(pos);
                    buf.set_selection(line.start, line.end);
                    self.last_activity = now;
                }
            } else if resp.clicked() {
                self.single_click(buf, layout, cx, cy, shift, now);
            }
        }
    }

    fn click_open_menu(
        &mut self,
        ui: &egui::Ui,
        buf: &mut Buffer,
        layout: &Layout,
        open: usize,
        cx: isize,
        cy: isize,
    ) {
        if let Some(i) = menu::item_at(open, layout.cols, cx, cy) {
            if let Some(cmd) = MENUS[open].items[i].cmd {
                self.menu = MenuState::Closed;
                self.run(ui, buf, cmd);
            }
            return;
        }
        if let Some(t) = menu::title_at(cx, cy) {
            // clicar no título aberto fecha; em outro título, troca de menu
            self.menu = if t == open {
                MenuState::Closed
            } else {
                MenuState::Open { menu: t, item: menu::first_item(t) }
            };
            return;
        }
        // clique na moldura ou num separador do próprio menu: ignora; fora dele: fecha
        if !menu::dropdown_rect(open, layout.cols).contains(cx, cy) {
            self.menu = MenuState::Closed;
        }
    }

    fn single_click(
        &mut self,
        buf: &mut Buffer,
        layout: &Layout,
        cx: isize,
        cy: isize,
        shift: bool,
        now: f64,
    ) {
        if let Some(t) = menu::title_at(cx, cy) {
            self.menu = MenuState::Open { menu: t, item: menu::first_item(t) };
            return;
        }
        let (vbar, hbar) = self.bars(buf, layout);
        if let Some(hit) = vbar.hit(cx, cy) {
            self.bar_click(hit, true, layout);
            return;
        }
        if let Some(hit) = hbar.hit(cx, cy) {
            self.bar_click(hit, false, layout);
            return;
        }
        if layout.text.contains(cx, cy) || layout.gutter.contains(cx, cy) {
            let pos = self.pos_from_cell(buf, layout, cx, cy);
            let anchor = if shift { buf.sel.anchor } else { pos };
            buf.set_selection(anchor, pos);
            self.last_activity = now;
        }
    }

    fn bar_click(&mut self, hit: BarHit, vertical: bool, layout: &Layout) {
        let view = if vertical { layout.text.h } else { layout.text.w };
        let page = view.saturating_sub(1).max(1);
        let pos = if vertical { &mut self.scroll_y } else { &mut self.scroll_x };
        match hit {
            BarHit::ArrowBack => *pos = pos.saturating_sub(1),
            BarHit::ArrowForward => *pos += 1,
            BarHit::PageBack => *pos = pos.saturating_sub(page),
            BarHit::PageForward => *pos += page,
            BarHit::Thumb => {}
        }
    }

    fn begin_drag(
        &mut self,
        buf: &mut Buffer,
        layout: &Layout,
        cx: isize,
        cy: isize,
        shift: bool,
        now: f64,
    ) {
        let (vbar, hbar) = self.bars(buf, layout);
        if vbar.hit(cx, cy) == Some(BarHit::Thumb) {
            // `grab`: em que ponto do polegar o mouse o agarrou
            let grab = vbar.track_offset(cx, cy) - vbar.thumb().0 as isize;
            self.drag = Drag::VThumb { grab };
        } else if hbar.hit(cx, cy) == Some(BarHit::Thumb) {
            let grab = hbar.track_offset(cx, cy) - hbar.thumb().0 as isize;
            self.drag = Drag::HThumb { grab };
        } else if layout.text.contains(cx, cy) || layout.gutter.contains(cx, cy) {
            let pos = self.pos_from_cell(buf, layout, cx, cy);
            let anchor = if shift { buf.sel.anchor } else { pos };
            buf.set_selection(anchor, pos);
            self.drag = Drag::Select;
            self.last_activity = now;
        }
    }

    fn update_drag(&mut self, buf: &mut Buffer, layout: &Layout, cx: isize, cy: isize, now: f64) {
        let (vbar, hbar) = self.bars(buf, layout);
        match self.drag {
            Drag::VThumb { grab } => {
                self.scroll_y = vbar.pos_for_thumb(vbar.track_offset(cx, cy) - grab);
            }
            Drag::HThumb { grab } => {
                self.scroll_x = hbar.pos_for_thumb(hbar.track_offset(cx, cy) - grab);
            }
            Drag::Select => {
                // Com o mouse fora da área de texto, a linha calculada cai fora da vista e
                // `reveal` rola até ela: é o autoscroll.
                let pos = self.pos_from_cell(buf, layout, cx, cy);
                let anchor = buf.sel.anchor;
                buf.set_selection(anchor, pos);
                self.touch(now);
            }
            Drag::Idle | Drag::PickerThumb { .. } => {}
        }
    }

    fn on_wheel(
        &mut self,
        unit: egui::MouseWheelUnit,
        delta: egui::Vec2,
        shift: bool,
        layout: &Layout,
        grid: &Grid,
    ) {
        let cells_per_unit = match unit {
            egui::MouseWheelUnit::Line => 3.0,
            egui::MouseWheelUnit::Page => layout.text.h as f32,
            _ => 1.0 / grid.rh, // pontos (touchpad) -> linhas
        };
        let (mut dx, mut dy) = (delta.x * cells_per_unit, delta.y * cells_per_unit);
        if shift && dx == 0.0 {
            dx = dy; // Shift + roda = rolagem horizontal
            dy = 0.0;
        }
        // `delta` positivo = o conteúdo desce (aparece o que estava acima): rola para cima.
        self.wheel.0 -= dx;
        self.wheel.1 -= dy;
        let (wx, wy) = (self.wheel.0.trunc(), self.wheel.1.trunc());
        self.wheel.0 -= wx;
        self.wheel.1 -= wy;
        if matches!(self.modal, Modal::Picker { .. }) {
            self.picker_wheel(wy as isize, layout); // a roda rola a lista do seletor
            return;
        }
        self.scroll_x = (self.scroll_x as isize + wx as isize).max(0) as usize;
        self.scroll_y = (self.scroll_y as isize + wy as isize).max(0) as usize;
    }

    // ----------------------------------------------------------------- teclado

    fn on_event(
        &mut self,
        ui: &egui::Ui,
        buf: &mut Buffer,
        ev: &egui::Event,
        layout: &Layout,
        grid: &Grid,
        now: f64,
    ) {
        use egui::Event;
        let picker_open = matches!(self.modal, Modal::Picker { .. });
        match ev {
            Event::Key { key, pressed: true, modifiers, .. } => {
                self.on_key(ui, buf, *key, *modifiers, layout, now);
            }
            Event::Text(text) if !text.chars().any(|c| c.is_control()) => {
                if picker_open {
                    self.modal_text(text);
                    self.last_activity = now;
                } else if !self.captured() {
                    buf.insert(text);
                    self.touch(now);
                }
            }
            Event::Paste(text) => {
                if picker_open {
                    self.modal_text(text.lines().next().unwrap_or(""));
                    self.last_activity = now;
                } else if !self.captured() {
                    let clean = text
                        .replace("\r\n", "\n")
                        .replace('\r', "\n")
                        .replace('\t', "    ");
                    buf.insert(&clean);
                    self.touch(now);
                }
            }
            Event::Copy if !self.captured() => self.copy(ui, buf),
            Event::Cut if !self.captured() => {
                self.cut(ui, buf);
                self.touch(now);
            }
            Event::MouseWheel { unit, delta, modifiers, .. } if picker_open || !self.captured() => {
                self.on_wheel(unit.clone(), *delta, modifiers.shift, layout, grid);
            }
            _ => {}
        }
    }

    fn on_key(
        &mut self,
        ui: &egui::Ui,
        buf: &mut Buffer,
        key: egui::Key,
        m: egui::Modifiers,
        layout: &Layout,
        now: f64,
    ) {
        use egui::Key;
        // Diálogo aberto: ele recebe todas as teclas.
        if !matches!(self.modal, Modal::None) {
            self.last_activity = now;
            self.modal_key(buf, key, m.command, m.shift, layout);
            return;
        }
        if let MenuState::Open { menu: open, item } = self.menu {
            self.on_menu_key(ui, buf, key, open, item);
            return;
        }
        match key {
            Key::F1 => {
                self.modal = Modal::About;
                return;
            }
            Key::F9 | Key::F10 => {
                self.menu = MenuState::Open { menu: 0, item: menu::first_item(0) };
                return;
            }
            _ => {}
        }
        if self.editor_key(buf, key, m, layout) {
            self.touch(now);
        }
    }

    fn on_menu_key(&mut self, ui: &egui::Ui, buf: &mut Buffer, key: egui::Key, open: usize, item: usize) {
        use egui::Key;
        let n = MENUS.len();
        match key {
            Key::Escape | Key::F9 | Key::F10 => self.menu = MenuState::Closed,
            Key::ArrowLeft => {
                let m = (open + n - 1) % n;
                self.menu = MenuState::Open { menu: m, item: menu::first_item(m) };
            }
            Key::ArrowRight => {
                let m = (open + 1) % n;
                self.menu = MenuState::Open { menu: m, item: menu::first_item(m) };
            }
            Key::ArrowUp => {
                self.menu = MenuState::Open { menu: open, item: menu::step_item(open, item, -1) };
            }
            Key::ArrowDown => {
                self.menu = MenuState::Open { menu: open, item: menu::step_item(open, item, 1) };
            }
            Key::Enter | Key::Space => {
                if let Some(cmd) = MENUS[open].items[item].cmd {
                    self.menu = MenuState::Closed;
                    self.run(ui, buf, cmd);
                }
            }
            _ => {}
        }
    }

    /// Teclas do editor. Devolve `true` se a tecla foi tratada.
    fn editor_key(
        &mut self,
        buf: &mut Buffer,
        key: egui::Key,
        m: egui::Modifiers,
        layout: &Layout,
    ) -> bool {
        use egui::Key;
        let shift = m.shift;
        let cmd = m.command; // Ctrl no Windows/Linux, Cmd no macOS
        let page = layout.text.h.saturating_sub(1).max(1) as isize;
        match key {
            Key::ArrowLeft if cmd => {
                let to = buf.word_left(buf.sel.head);
                buf.move_head(to, shift);
            }
            Key::ArrowRight if cmd => {
                let to = buf.word_right(buf.sel.head);
                buf.move_head(to, shift);
            }
            Key::ArrowLeft => buf.move_left(shift),
            Key::ArrowRight => buf.move_right(shift),
            Key::ArrowUp => buf.move_vertical(-1, shift),
            Key::ArrowDown => buf.move_vertical(1, shift),
            Key::PageUp => buf.move_vertical(-page, shift),
            Key::PageDown => buf.move_vertical(page, shift),
            Key::Home if cmd => buf.move_head(0, shift),
            Key::Home => buf.move_line_start(shift),
            Key::End if cmd => {
                let end = buf.len_chars();
                buf.move_head(end, shift);
            }
            Key::End => buf.move_line_end(shift),
            Key::Backspace => buf.backspace(),
            Key::Delete => buf.delete_forward(),
            Key::Enter => buf.newline_with_indent(),
            Key::Tab => buf.insert("    "),
            Key::A if cmd => buf.select_all(),
            Key::Z if cmd && shift => buf.redo(),
            Key::Z if cmd => buf.undo(),
            Key::Y if cmd => buf.redo(),
            Key::N if cmd => self.file_cmd(buf, Cmd::New),
            Key::O if cmd => self.file_cmd(buf, Cmd::Open),
            Key::S if cmd && shift => self.file_cmd(buf, Cmd::SaveAs),
            Key::S if cmd => self.file_cmd(buf, Cmd::Save),
            Key::Q if cmd => self.file_cmd(buf, Cmd::Quit),
            Key::F2 => self.palette = (self.palette + 1) % PALETTES.len(),
            Key::F3 => self.unicode = !self.unicode,
            _ => return false,
        }
        true
    }

    fn copy(&self, ui: &egui::Ui, buf: &Buffer) {
        let selected = buf.selected_text();
        if !selected.is_empty() {
            ui.copy_text(selected);
        }
    }

    fn cut(&mut self, ui: &egui::Ui, buf: &mut Buffer) {
        self.copy(ui, buf);
        if !buf.sel.is_empty() {
            buf.insert("");
            self.reveal = true;
        }
    }

    /// Executa um comando de menu.
    fn run(&mut self, ui: &egui::Ui, buf: &mut Buffer, cmd: Cmd) {
        match cmd {
            Cmd::New | Cmd::Open | Cmd::Save | Cmd::SaveAs | Cmd::Quit => {
                self.file_cmd(buf, cmd);
            }
            Cmd::Undo => {
                buf.undo();
                self.reveal = true;
            }
            Cmd::Redo => {
                buf.redo();
                self.reveal = true;
            }
            Cmd::Cut => self.cut(ui, buf),
            Cmd::Copy => self.copy(ui, buf),
            Cmd::SelectAll => buf.select_all(),
            Cmd::ToggleNumbers => self.line_numbers = !self.line_numbers,
            Cmd::ToggleUnicode => self.unicode = !self.unicode,
            Cmd::Palette(i) => self.palette = i.min(PALETTES.len() - 1),
            Cmd::About => self.modal = Modal::About,
        }
    }

    // ---------------------------------------------------------------- desenho

    /// Monta a grade de células. Ordem = camadas, de baixo para cima.
    fn compose(
        &self,
        s: &mut Screen,
        buf: &Buffer,
        layout: &Layout,
        look: &Look,
        focused: bool,
        now: f64,
    ) {
        s.reset(layout.cols, layout.rows, look.p.text_fg, look.p.desktop);
        self.draw_window(s, look, layout, buf);
        self.draw_editor(s, look, buf, layout, focused, now);
        self.draw_menu_bar(s, look);
        self.draw_status(s, look, layout, buf);
        if let MenuState::Open { menu: open, item } = self.menu {
            self.draw_dropdown(s, look, layout, open, item);
        }
        let caret_on = ((now - self.last_activity) * 2.0).floor() as i64 % 2 == 0;
        self.draw_modal(s, look, layout, caret_on);
    }

    fn draw_menu_bar(&self, s: &mut Screen, look: &Look) {
        let p = &look.p;
        for x in 0..s.cols {
            s.put(x, 0, ' ', p.bar_fg, p.bar_bg);
        }
        for (i, r) in menu::title_rects().iter().enumerate() {
            let open = matches!(self.menu, MenuState::Open { menu, .. } if menu == i);
            let (fg, bg) = if open {
                (p.pick_fg, p.pick_bg)
            } else {
                (p.bar_fg, p.bar_bg)
            };
            s.fill(*r, ' ', fg, bg);
            let title = MENUS[i].title;
            s.text(r.x + 1, 0, title, fg, bg);
            // a inicial fica em destaque, como nas TUIs clássicas
            if let Some(first) = title.chars().next() {
                s.put(r.x + 1, 0, first, if open { fg } else { p.hot }, bg);
            }
        }
        let name = format!("{} ", PALETTES[self.palette].name);
        let x = s.cols.saturating_sub(name.chars().count());
        s.text(x, 0, &name, p.bar_fg, p.bar_bg);
    }

    /// Moldura dupla da janela, título centrado e indicador de posição.
    fn draw_window(&self, s: &mut Screen, look: &Look, layout: &Layout, buf: &Buffer) {
        let (p, g) = (&look.p, &look.g);
        let w = layout.window;
        s.fill(w, ' ', p.text_fg, p.text_bg);
        s.frame(w, g, true, p.frame_fg, p.text_bg);
        let star = if buf.is_dirty() { "*" } else { "" };
        let label = format!(" {}{} ", self.file.display_name(), star);
        let lx = w.x + w.w.saturating_sub(label.chars().count()) / 2;
        s.text(lx, w.y, &label, p.title_fg, p.text_bg);

        let (line, col) = buf.line_col(buf.sel.head);
        let pos: String = format!("[ {:>4}:{:<4} ]", line + 1, col + 1)
            .chars()
            .take(POS_LABEL_W)
            .collect();
        s.text(2, w.bottom() - 1, &pos, p.title_fg, p.text_bg);
    }

    /// Números de linha, texto com realce, seleção, cursor e barras de rolagem.
    fn draw_editor(
        &self,
        s: &mut Screen,
        look: &Look,
        buf: &Buffer,
        layout: &Layout,
        focused: bool,
        now: f64,
    ) {
        let (p, g) = (&look.p, &look.g);
        let text = layout.text;
        let n_lines = buf.line_count();
        let sel = buf.sel.range();
        let (caret_line, caret_col) = buf.line_col(buf.sel.head);
        let digits = layout.gutter.w.saturating_sub(2);

        for row in 0..text.h {
            let line = self.scroll_y + row;
            let y = text.y + row;
            if layout.gutter.w > 0 {
                s.put(layout.gutter.right() - 1, y, g.v, p.dim, p.text_bg);
            }
            if line >= n_lines {
                continue;
            }
            if layout.gutter.w > 0 {
                let num = format!("{:>width$} ", line + 1, width = digits);
                let fg = if line == caret_line { p.title_fg } else { p.dim };
                s.text(layout.gutter.x, y, &num, fg, p.text_bg);
            }

            // Texto com realce: só as colunas visíveis chegam à grade.
            let line_text = buf.line_text(line);
            let mut col = 0usize;
            for (range, token) in tokenize(&line_text) {
                let fg = token_color(p, token);
                for ch in line_text[range].chars() {
                    if col >= self.scroll_x && col - self.scroll_x < text.w {
                        s.put(text.x + col - self.scroll_x, y, ch, fg, p.text_bg);
                    }
                    col += 1;
                }
            }

            // Seleção: pinta as células selecionadas (inclui a célula do '\n').
            let ls = buf.line_start(line);
            let len = buf.line_len(line);
            if !sel.is_empty() && sel.end > ls && sel.start <= ls + len {
                let c0 = sel.start.max(ls) - ls;
                let c1 = sel.end.min(ls + len + 1) - ls;
                for c in c0..c1 {
                    if c >= self.scroll_x && c - self.scroll_x < text.w {
                        let x = text.x + c - self.scroll_x;
                        let ch = s.get(x, y).map_or(' ', |cell| cell.ch);
                        s.put(x, y, ch, p.sel_fg, p.sel_bg);
                    }
                }
            }
        }

        // Cursor: bloco com as cores invertidas, piscando (a fase reinicia a cada ação).
        let blink_on = ((now - self.last_activity) * 2.0).floor() as i64 % 2 == 0;
        let in_view = caret_line >= self.scroll_y
            && caret_line < self.scroll_y + text.h
            && caret_col >= self.scroll_x
            && caret_col < self.scroll_x + text.w;
        if focused && blink_on && in_view {
            let x = text.x + caret_col - self.scroll_x;
            let y = text.y + caret_line - self.scroll_y;
            let ch = s.get(x, y).map_or(' ', |cell| cell.ch);
            s.put(x, y, ch, p.text_bg, p.caret);
        }

        // Barras de rolagem (sobre a moldura).
        let colors = BarColors {
            arrow: p.frame_fg,
            track: p.dim,
            thumb: p.frame_fg,
            bg: p.text_bg,
        };
        let (vbar, hbar) = self.bars(buf, layout);
        vbar.draw(s, g, &colors);
        hbar.draw(s, g, &colors);
    }

    fn draw_status(&self, s: &mut Screen, look: &Look, layout: &Layout, buf: &Buffer) {
        let p = &look.p;
        let y = layout.rows - 1;
        for x in 0..layout.cols {
            s.put(x, y, ' ', p.bar_fg, p.bar_bg);
        }
        // Um aviso recente (arquivo salvo, erro...) ocupa o lugar das dicas.
        if let Some((message, until)) = &self.message {
            if self.now < *until {
                let text = ellipsize_end(message, layout.cols.saturating_sub(2));
                s.text(1, y, &text, p.bar_fg, p.bar_bg);
                return;
            }
        }
        let hints = [
            ("F1", "Sobre"),
            ("F10", "Menu"),
            ("Ctrl+O", "Abrir"),
            ("Ctrl+S", "Salvar"),
            ("Ctrl+Q", "Sair"),
        ];
        let mut x = 1;
        for (key, label) in hints {
            x += s.text(x, y, key, p.hot, p.bar_bg);
            x += s.text(x, y, &format!(" {}  ", label), p.bar_fg, p.bar_bg);
        }
        let info = format!("{} linhas ", buf.line_count());
        let ix = layout.cols.saturating_sub(info.chars().count());
        s.text(ix, y, &info, p.bar_fg, p.bar_bg);
    }

    /// Marca `[x] ` / `(*) ` dos itens de menu que são liga/desliga ou de escolha única.
    fn mark(&self, cmd: Cmd) -> String {
        match cmd {
            Cmd::ToggleNumbers => format!("[{}] ", if self.line_numbers { 'x' } else { ' ' }),
            Cmd::ToggleUnicode => format!("[{}] ", if self.unicode { 'x' } else { ' ' }),
            Cmd::Palette(i) => format!("({}) ", if self.palette == i { '*' } else { ' ' }),
            _ => "    ".to_string(),
        }
    }

    fn draw_dropdown(
        &self,
        s: &mut Screen,
        look: &Look,
        layout: &Layout,
        open: usize,
        selected: usize,
    ) {
        let (p, g) = (&look.p, &look.g);
        let r = menu::dropdown_rect(open, layout.cols);
        s.shadow(r, p.shadow_fg, p.shadow_bg);
        s.fill(r, ' ', p.menu_fg, p.menu_bg);
        s.frame(r, g, false, p.menu_fg, p.menu_bg);

        let marks = menu::has_marks(open);
        for (i, it) in MENUS[open].items.iter().enumerate() {
            let y = r.y + 1 + i;
            match it.cmd {
                None => {
                    s.put(r.x, y, g.tee_l, p.menu_fg, p.menu_bg);
                    for x in r.x + 1..r.right() - 1 {
                        s.put(x, y, g.h, p.menu_fg, p.menu_bg);
                    }
                    s.put(r.right() - 1, y, g.tee_r, p.menu_fg, p.menu_bg);
                }
                Some(cmd) => {
                    let picked = i == selected;
                    let (fg, bg) = if picked {
                        (p.pick_fg, p.pick_bg)
                    } else {
                        (p.menu_fg, p.menu_bg)
                    };
                    for x in r.x + 1..r.right() - 1 {
                        s.put(x, y, ' ', fg, bg);
                    }
                    let mut label = String::new();
                    if marks {
                        label.push_str(&self.mark(cmd));
                    }
                    label.push_str(it.label);
                    s.text(r.x + 2, y, &label, fg, bg);
                    if !it.keys.is_empty() {
                        let kx = r.right() - 2 - it.keys.chars().count();
                        s.text(kx, y, it.keys, if picked { fg } else { p.hot }, bg);
                    }
                }
            }
        }
    }

    /// Diálogo "Sobre": o logo em ASCII art, uma legenda de atalhos e o botão OK.
    fn draw_about(&self, s: &mut Screen, look: &Look, layout: &Layout) {
        let (p, g) = (&look.p, &look.g);
        let r = layout.about;
        s.shadow(r, p.shadow_fg, p.shadow_bg);
        s.fill(r, ' ', p.menu_fg, p.menu_bg);
        s.frame(r, g, true, p.menu_fg, p.menu_bg);
        s.text_centered(r, r.y, " Sobre ", p.hot, p.menu_bg);

        let on = if self.unicode { '█' } else { '#' };
        for (i, line) in banner::mini_edit(on).iter().enumerate() {
            s.text_centered(r, r.y + 2 + i, line, p.accent, p.menu_bg);
        }
        let lines = [
            (8, "Editor de texto em Rust + egui"),
            (9, "Rope (ropey) - undo/redo - abrir e salvar arquivos"),
            (11, "Ctrl+O Abrir  Ctrl+S Salvar  Ctrl+Shift+S Salvar como"),
            (12, "F10 Menu   F2 Paleta   F3 Bordas   Ctrl+Q Sair"),
        ];
        for (dy, text) in lines {
            s.text_centered(r, r.y + dy, text, p.menu_fg, p.menu_bg);
        }

        let ok = layout.about_ok();
        s.fill(ok, ' ', p.pick_fg, p.pick_bg);
        s.text(ok.x, ok.y, BUTTON, p.pick_fg, p.pick_bg);
    }
}

fn token_color(p: &Palette, t: Token) -> Color32 {
    match t {
        Token::Plain => p.text_fg,
        Token::Keyword => p.keyword,
        Token::Type => p.ty,
        Token::Macro => p.mac,
        Token::Str => p.string,
        Token::Number => p.number,
        Token::Comment => p.comment,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_for_a_typical_window() {
        let l = Layout::new(106, 34, 22, true);
        assert_eq!(l.window, CellRect::new(0, 1, 106, 32));
        assert_eq!(l.gutter, CellRect::new(1, 2, 4, 30));
        assert_eq!(l.text, CellRect::new(5, 2, 100, 30));
        assert_eq!(l.vbar, CellRect::new(105, 2, 1, 30));
        assert_eq!(l.hbar, CellRect::new(16, 32, 89, 1));
        assert_eq!(l.about, CellRect::new(25, 9, 56, 16));
        assert_eq!(l.about_ok(), CellRect::new(49, 23, 8, 1));
    }

    #[test]
    fn layout_without_line_numbers_gives_the_width_back() {
        let l = Layout::new(106, 34, 22, false);
        assert_eq!(l.gutter.w, 0);
        assert_eq!(l.text, CellRect::new(1, 2, 104, 30));
    }

    #[test]
    fn gutter_grows_with_the_number_of_digits() {
        let l = Layout::new(200, 60, 12345, true);
        assert_eq!(l.gutter.w, 7);
        assert_eq!(l.text.x, 8);
    }

    #[test]
    fn minimum_window_still_lays_out() {
        let l = Layout::new(MIN_COLS, MIN_ROWS, 22, true);
        assert_eq!(l.text, CellRect::new(5, 2, 58, 16));
        assert_eq!(l.about, CellRect::new(4, 2, 56, 16));
    }

    #[test]
    fn caret_is_revealed_when_it_leaves_the_view() {
        let text: String = (0..100).map(|i| format!("linha {i}\n")).collect();
        let mut buf = Buffer::new(&text);
        let mut ui = AsciiUi::new();
        let layout = Layout::new(106, 34, buf.line_count(), true); // 30 linhas visíveis

        let p = buf.pos_at(50, 0);
        buf.set_selection(p, p);
        ui.reveal_caret(&buf, &layout);
        assert_eq!(ui.scroll_y, 50 + 1 - 30); // a linha 50 vira a última visível

        buf.set_selection(0, 0);
        ui.reveal_caret(&buf, &layout);
        assert_eq!(ui.scroll_y, 0);
    }

    #[test]
    fn scroll_is_clamped_to_the_content() {
        let buf = Buffer::new("a\nb\nc"); // cabe inteiro na tela
        let mut ui = AsciiUi::new();
        let layout = Layout::new(106, 34, buf.line_count(), true);
        ui.scroll_y = 10;
        ui.scroll_x = 10;
        ui.clamp_scroll(&buf, &layout);
        assert_eq!((ui.scroll_y, ui.scroll_x), (0, 0));
    }

    #[test]
    fn wheel_scrolls_three_lines_per_notch() {
        let mut ui = AsciiUi::new();
        let layout = Layout::new(106, 34, 100, true);
        let grid = Grid { origin: egui::pos2(0.0, 0.0), cw: 9.0, rh: 18.0 };
        ui.scroll_y = 10;
        // delta positivo = roda para cima = mostra o que está acima
        ui.on_wheel(egui::MouseWheelUnit::Line, egui::vec2(0.0, 1.0), false, &layout, &grid);
        assert_eq!(ui.scroll_y, 7);
        ui.on_wheel(egui::MouseWheelUnit::Line, egui::vec2(0.0, -1.0), false, &layout, &grid);
        assert_eq!(ui.scroll_y, 10);
    }

    #[test]
    fn pixel_to_cell_conversion() {
        let grid = Grid { origin: egui::pos2(10.0, 20.0), cw: 9.0, rh: 18.0 };
        assert_eq!(grid.cell_at(egui::pos2(10.0, 20.0)), (0, 0));
        assert_eq!(grid.cell_at(egui::pos2(28.5, 56.0)), (2, 2));
        assert_eq!(grid.cell_at(egui::pos2(0.0, 0.0)), (-2, -2)); // fora da grade: negativo
    }
}
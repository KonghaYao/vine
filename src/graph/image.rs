use std::{
    fmt::{self, Debug, Formatter},
    hash::{Hash, Hasher},
    io::Cursor,
    process,
    time::{SystemTime, UNIX_EPOCH},
};

use ratatui::style::{Color, Style};
use rustc_hash::{FxHashMap, FxHashSet};

use crate::{
    color::GraphColorSet,
    git::CommitHash,
    graph::{
        geometry::{bounding_box_u32, Point},
        Edge, EdgeType, Graph,
    },
    protocol::{ImageProtocol, PreparedImage, PreparedImageCell},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GraphStyle {
    Rounded,
    Angular,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GraphImageWidthMode {
    Compact,
    Fixed,
}

#[derive(Debug)]
pub struct GraphImageManager<'a> {
    prepared_image_map: FxHashMap<CommitHash, PreparedImage>,
    image_ids: FxHashSet<u32>,
    pending_uploads: Vec<String>,

    graph: &'a Graph<'a>,
    cell_width_type: CellWidthType,
    graph_style: GraphStyle,
    image_width_mode: GraphImageWidthMode,
    image_params: ImageParams,
    drawing_pixels: DrawingPixels,
    image_protocol: ImageProtocol,
    session_nonce: u32,
}

impl<'a> GraphImageManager<'a> {
    pub fn new(
        graph: &'a Graph,
        graph_color_set: &GraphColorSet,
        cell_width_type: CellWidthType,
        graph_style: GraphStyle,
        image_width_mode: GraphImageWidthMode,
        image_protocol: ImageProtocol,
    ) -> Self {
        let image_params = ImageParams::new(graph_color_set, cell_width_type);
        let drawing_pixels = DrawingPixels::new(&image_params);

        GraphImageManager {
            prepared_image_map: FxHashMap::default(),
            image_ids: FxHashSet::default(),
            pending_uploads: Vec::default(),
            graph,
            cell_width_type,
            graph_style,
            image_width_mode,
            image_params,
            drawing_pixels,
            image_protocol,
            session_nonce: create_session_nonce(),
        }
    }

    pub fn prepared_image(&self, commit_hash: &CommitHash) -> &PreparedImage {
        self.prepared_image_map.get(commit_hash).unwrap()
    }

    pub fn graph(&self) -> &'a Graph<'a> {
        self.graph
    }

    pub fn graph_cell_width(&self) -> u16 {
        let cell_count = (self.graph.max_pos_x + 1) as u16;
        match self.cell_width_type {
            CellWidthType::Double => cell_count * 2,
            CellWidthType::Single => cell_count,
        }
    }

    pub fn image_ids(&self) -> &FxHashSet<u32> {
        &self.image_ids
    }

    pub fn drain_pending_uploads(&mut self) -> Vec<String> {
        std::mem::take(&mut self.pending_uploads)
    }

    pub fn ensure_uploaded(&mut self, commit_hash: &CommitHash) {
        if self.prepared_image_map.contains_key(commit_hash) {
            return;
        }
        let image = if matches!(self.image_protocol, ImageProtocol::Ascii) {
            build_ascii_prepared_image(
                self.graph,
                &self.image_params,
                self.graph_style,
                self.image_width_mode,
                self.cell_width_type,
                commit_hash,
            )
        } else {
            let graph_row_image = build_single_graph_row_image(
                self.graph,
                &self.image_params,
                &self.drawing_pixels,
                self.graph_style,
                self.image_width_mode,
                commit_hash,
            );
            let image_id = graph_image_id(self.session_nonce, commit_hash);
            let mut image =
                graph_row_image.prepare(self.cell_width_type, self.image_protocol, image_id);
            if let Some(upload_data) = image.take_upload_data() {
                self.pending_uploads.push(upload_data);
            }
            self.image_ids.insert(image_id);
            image
        };
        self.prepared_image_map.insert(commit_hash.clone(), image);
    }
}

pub struct GraphRowImage {
    pub bytes: Vec<u8>,
    pub cell_count: usize,
}

impl Debug for GraphRowImage {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "GraphRowImage {{ bytes: [{} bytes], cell_count: {} }}",
            self.bytes.len(),
            self.cell_count
        )
    }
}

impl GraphRowImage {
    fn prepare(
        &self,
        cell_width_type: CellWidthType,
        image_protocol: ImageProtocol,
        image_id: u32,
    ) -> PreparedImage {
        let image_cell_width = match cell_width_type {
            CellWidthType::Double => self.cell_count * 2,
            CellWidthType::Single => self.cell_count,
        };
        image_protocol.prepare_image(&self.bytes, image_cell_width, image_id)
    }
}

fn create_session_nonce() -> u32 {
    let mut hasher = rustc_hash::FxHasher::default();
    process::id().hash(&mut hasher);
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
        .hash(&mut hasher);
    hasher.finish() as u32
}

fn graph_image_id(session_nonce: u32, commit_hash: &CommitHash) -> u32 {
    let mut hasher = rustc_hash::FxHasher::default();
    session_nonce.hash(&mut hasher);
    commit_hash.hash(&mut hasher);
    hasher.finish() as u32
}

#[derive(Debug)]
pub struct ImageParams {
    width: u16,
    height: u16,
    line_width: u16,
    circle_inner_radius: u16,
    circle_outer_radius: u16,
    edge_colors: Vec<image::Rgba<u8>>,
    circle_edge_color: image::Rgba<u8>,
    background_color: image::Rgba<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CellWidthType {
    Double, // 2 cells
    Single,
}

impl ImageParams {
    pub fn new(graph_color_set: &GraphColorSet, cell_width_type: CellWidthType) -> Self {
        let (width, height, line_width, circle_inner_radius, circle_outer_radius) =
            match cell_width_type {
                CellWidthType::Double => (50, 50, 5, 10, 13),
                CellWidthType::Single => (25, 50, 3, 7, 10),
            };
        let edge_colors = graph_color_set
            .colors
            .iter()
            .map(|c| c.to_image_color())
            .collect();
        let circle_edge_color = graph_color_set.edge_color.to_image_color();
        let background_color = graph_color_set.background_color.to_image_color();
        Self {
            width,
            height,
            line_width,
            circle_inner_radius,
            circle_outer_radius,
            edge_colors,
            circle_edge_color,
            background_color,
        }
    }

    fn edge_color(&self, index: usize) -> image::Rgba<u8> {
        self.edge_colors[index % self.edge_colors.len()]
    }

    fn corner_radius(&self) -> u16 {
        if self.width < self.height {
            self.width / 2
        } else {
            self.height / 2
        }
    }
}

fn build_single_graph_row_image(
    graph: &Graph<'_>,
    image_params: &ImageParams,
    drawing_pixels: &DrawingPixels,
    graph_style: GraphStyle,
    image_width_mode: GraphImageWidthMode,
    commit_hash: &CommitHash,
) -> GraphRowImage {
    let (pos_x, pos_y) = graph.commit_pos_map[&commit_hash];
    let edges = &graph.edges[pos_y];

    let max_pos_x = match image_width_mode {
        GraphImageWidthMode::Compact => edges.iter().map(|e| e.pos_x).fold(pos_x, usize::max),
        GraphImageWidthMode::Fixed => graph.max_pos_x,
    };

    let cell_count = max_pos_x + 1;

    calc_graph_row_image(
        pos_x,
        cell_count,
        edges,
        image_params,
        drawing_pixels,
        graph_style,
    )
}

// ASCII graph rendering
//
// The ASCII graph reuses the `PreparedImage` cell structure: each cell of a row holds a
// box-drawing character colored with the same palette as the rendered image. One graph column
// occupies one cell, or two cells when the cell width type is double; the second cell is a
// filler that carries the horizontal lines across the gap between the columns.

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct AsciiDirections {
    up: bool,
    down: bool,
    left: bool,
    right: bool,
}

fn ascii_symbol(d: AsciiDirections, graph_style: GraphStyle) -> char {
    // T-junctions and the cross are the same in both styles; only the four single corners have
    // a rounded variant (╭╮╰╯) and an angular one (┌┐└┘).
    let (top_left, top_right, bottom_left, bottom_right) = match graph_style {
        GraphStyle::Rounded => ('\u{256d}', '\u{256e}', '\u{2570}', '\u{256f}'),
        GraphStyle::Angular => ('\u{250c}', '\u{2510}', '\u{2514}', '\u{2518}'),
    };
    match (d.up, d.down, d.left, d.right) {
        (true, true, true, true) => '\u{253c}',     // ┼
        (true, true, true, false) => '\u{2524}',    // ┤
        (true, true, false, true) => '\u{251c}',    // ├
        (true, false, true, true) => '\u{2534}',    // ┴
        (false, true, true, true) => '\u{252c}',    // ┬
        (true, false, false, true) => bottom_left,  // ╰ / └
        (true, false, true, false) => bottom_right, // ╯ / ┘
        (false, true, false, true) => top_left,     // ╭ / ┌
        (false, true, true, false) => top_right,    // ╮ / ┐
        // A single direction is a line that terminates in this cell, so it is drawn as a full
        // vertical or horizontal line.
        (true, true, false, false) | (true, false, false, false) | (false, true, false, false) => {
            '\u{2502}' // │
        }
        (false, false, true, true) | (false, false, true, false) | (false, false, false, true) => {
            '\u{2500}' // ─
        }
        (false, false, false, false) => ' ',
    }
}

fn edge_directions(edge_type: EdgeType) -> AsciiDirections {
    let mut d = AsciiDirections::default();
    match edge_type {
        EdgeType::Vertical => {
            d.up = true;
            d.down = true;
        }
        EdgeType::Horizontal => {
            d.left = true;
            d.right = true;
        }
        EdgeType::Up => d.up = true,
        EdgeType::Down => d.down = true,
        EdgeType::Left => d.left = true,
        EdgeType::Right => d.right = true,
        // The corner edges are named after the corner that the curve occupies in the cell:
        // ╭ (LeftTop) joins down and right, ╮ (RightTop) down and left,
        // ╰ (LeftBottom) up and right, ╯ (RightBottom) up and left.
        EdgeType::LeftTop => {
            d.down = true;
            d.right = true;
        }
        EdgeType::RightTop => {
            d.down = true;
            d.left = true;
        }
        EdgeType::LeftBottom => {
            d.up = true;
            d.right = true;
        }
        EdgeType::RightBottom => {
            d.up = true;
            d.left = true;
        }
    }
    d
}

fn image_color_to_ratatui(color: image::Rgba<u8>) -> Color {
    Color::Rgb(color[0], color[1], color[2])
}

fn ascii_style(color: Option<Color>) -> Style {
    match color {
        Some(color) => Style::default().fg(color),
        None => Style::default(),
    }
}

struct AsciiCell {
    dirs: AsciiDirections,
    color: Option<Color>,
}

fn build_ascii_prepared_image(
    graph: &Graph<'_>,
    image_params: &ImageParams,
    graph_style: GraphStyle,
    image_width_mode: GraphImageWidthMode,
    cell_width_type: CellWidthType,
    commit_hash: &CommitHash,
) -> PreparedImage {
    let (pos_x, pos_y) = graph.commit_pos_map[&commit_hash];
    let edges = &graph.edges[pos_y];

    let max_pos_x = match image_width_mode {
        GraphImageWidthMode::Compact => edges.iter().map(|e| e.pos_x).fold(pos_x, usize::max),
        GraphImageWidthMode::Fixed => graph.max_pos_x,
    };

    render_ascii_row(
        pos_x,
        max_pos_x + 1,
        edges,
        image_params,
        graph_style,
        cell_width_type,
    )
}

fn render_ascii_row(
    pos_x: usize,
    cell_count: usize,
    edges: &[Edge],
    image_params: &ImageParams,
    graph_style: GraphStyle,
    cell_width_type: CellWidthType,
) -> PreparedImage {
    let columns: Vec<AsciiCell> = (0..cell_count)
        .map(|x| {
            let mut dirs = AsciiDirections::default();
            let mut vertical_color = None;
            let mut other_color = None;
            for edge in edges.iter().filter(|e| e.pos_x == x) {
                let d = edge_directions(edge.edge_type);
                dirs.up |= d.up;
                dirs.down |= d.down;
                dirs.left |= d.left;
                dirs.right |= d.right;

                // Prefer the color of a vertical line, since that is the branch the eye
                // follows through the cell, whereas a horizontal line only passes over it.
                let color = Some(image_color_to_ratatui(
                    image_params.edge_color(edge.associated_line_pos_x),
                ));
                if edge.edge_type.is_vertically_related() {
                    vertical_color = vertical_color.or(color);
                } else {
                    other_color = other_color.or(color);
                }
            }
            AsciiCell {
                dirs,
                color: vertical_color.or(other_color),
            }
        })
        .collect();

    // A horizontal line on a row comes either from a parent merging up into this commit or from
    // a child branching off this commit, and the corner type tells them apart:
    //   merge      → ╭ (LeftTop) / ╮ (RightTop), the line comes from a parent below
    //   branch off → ╰ (LeftBottom) / ╯ (RightBottom), the line goes up to a child above
    let is_merge_at_row = edges
        .iter()
        .any(|e| matches!(e.edge_type, EdgeType::LeftTop | EdgeType::RightTop));
    let merge_from_left = is_merge_at_row && columns[pos_x].dirs.left;
    let merge_from_right = is_merge_at_row && columns[pos_x].dirs.right;

    let commit_style = Style::default().fg(image_color_to_ratatui(image_params.edge_color(pos_x)));
    // A merge commit is drawn as an open circle, so that the arrows pointing into it read as the
    // directions the merged branches come from.
    let commit_symbol = if is_merge_at_row {
        '\u{25cb}' // ○
    } else {
        '\u{25cf}' // ●
    };

    let double_width = matches!(cell_width_type, CellWidthType::Double);
    let mut cells = Vec::with_capacity(if double_width {
        cell_count * 2
    } else {
        cell_count
    });
    for (x, col) in columns.iter().enumerate() {
        if x == pos_x {
            cells.push(PreparedImageCell::new(
                commit_symbol.to_string(),
                commit_style,
            ));
        } else {
            let symbol = ascii_symbol(col.dirs, graph_style);
            cells.push(PreparedImageCell::new(
                symbol.to_string(),
                ascii_style(col.color),
            ));
        }

        if !double_width {
            continue;
        }

        // The filler cell spans the second half of the graph column. It carries the horizontal
        // lines across the gap between the columns, and an arrow that points into a merge commit
        // from the adjacent column (`>` from the left, `<` from the right).
        let arrow_from_left = merge_from_left && x + 1 == pos_x;
        let arrow_from_right = merge_from_right && x == pos_x;
        let has_horizontal = col.dirs.right || (x + 1 < cell_count && columns[x + 1].dirs.left);
        if arrow_from_left {
            cells.push(PreparedImageCell::new(">".to_string(), commit_style));
        } else if arrow_from_right {
            cells.push(PreparedImageCell::new("<".to_string(), commit_style));
        } else if has_horizontal {
            let color = col
                .color
                .or_else(|| columns.get(x + 1).and_then(|c| c.color));
            cells.push(PreparedImageCell::new(
                '\u{2500}'.to_string(), // ─
                ascii_style(color),
            ));
        } else {
            cells.push(PreparedImageCell::new(" ".to_string(), Style::default()));
        }
    }

    PreparedImage::from_cells(cells)
}

type Pixels = FxHashSet<(i32, i32)>;

#[derive(Debug)]
pub struct DrawingPixels {
    circle: Pixels,
    circle_edge: Pixels,
    vertical_edge: Pixels,
    horizontal_edge: Pixels,
    up_edge: Pixels,
    down_edge: Pixels,
    left_edge: Pixels,
    right_edge: Pixels,
    right_top_edge: Pixels,
    left_top_edge: Pixels,
    right_bottom_edge: Pixels,
    left_bottom_edge: Pixels,
}

impl DrawingPixels {
    pub fn new(image_params: &ImageParams) -> Self {
        let circle = calc_commit_circle_drawing_pixels(image_params);
        let circle_edge = calc_circle_edge_drawing_pixels(image_params);
        let vertical_edge = calc_vertical_edge_drawing_pixels(image_params);
        let horizontal_edge = calc_horizontal_edge_drawing_pixels(image_params);
        let up_edge = calc_up_edge_drawing_pixels(image_params);
        let down_edge = calc_down_edge_drawing_pixels(image_params);
        let left_edge = calc_left_edge_drawing_pixels(image_params);
        let right_edge = calc_right_edge_drawing_pixels(image_params);
        let right_top_edge = calc_right_top_edge_drawing_pixels(image_params);
        let left_top_edge = calc_left_top_edge_drawing_pixels(image_params);
        let right_bottom_edge = calc_right_bottom_edge_drawing_pixels(image_params);
        let left_bottom_edge = calc_left_bottom_edge_drawing_pixels(image_params);

        Self {
            circle,
            circle_edge,
            vertical_edge,
            horizontal_edge,
            up_edge,
            down_edge,
            left_edge,
            right_edge,
            right_top_edge,
            left_top_edge,
            right_bottom_edge,
            left_bottom_edge,
        }
    }
}

fn calc_commit_circle_drawing_pixels(image_params: &ImageParams) -> Pixels {
    calc_circle_drawing_pixels(image_params, image_params.circle_inner_radius as i32)
}

fn calc_circle_edge_drawing_pixels(image_params: &ImageParams) -> Pixels {
    let inner = calc_circle_drawing_pixels(image_params, image_params.circle_inner_radius as i32);
    let outer = calc_circle_drawing_pixels(image_params, image_params.circle_outer_radius as i32);

    outer.difference(&inner).cloned().collect()
}

fn calc_circle_drawing_pixels(image_params: &ImageParams, radius: i32) -> Pixels {
    // Bresenham's circle algorithm
    let center_x = (image_params.width / 2) as i32;
    let center_y = (image_params.height / 2) as i32;

    let mut x = radius;
    let mut y = 0;
    let mut p = 1 - radius;

    let mut pixels = Pixels::default();

    while x >= y {
        for dx in -x..=x {
            pixels.insert((center_x + dx, center_y + y));
            pixels.insert((center_x + dx, center_y - y));
        }
        for dx in -y..=y {
            pixels.insert((center_x + dx, center_y + x));
            pixels.insert((center_x + dx, center_y - x));
        }

        y += 1;
        if p <= 0 {
            p += 2 * y + 1;
        } else {
            x -= 1;
            p += 2 * y - 2 * x + 1;
        }
    }

    pixels
}

fn calc_vertical_edge_drawing_pixels(image_params: &ImageParams) -> Pixels {
    let center_x = (image_params.width / 2) as i32;
    let line_width = image_params.line_width as i32;
    let x_start = center_x - line_width / 2;

    let mut pixels = Pixels::default();
    for y in 0..image_params.height as i32 {
        for x in x_start..(x_start + line_width) {
            pixels.insert((x, y));
        }
    }
    pixels
}

fn calc_horizontal_edge_drawing_pixels(image_params: &ImageParams) -> Pixels {
    let center_y = (image_params.height / 2) as i32;
    let line_width = image_params.line_width as i32;
    let y_start = center_y - line_width / 2;

    let mut pixels = Pixels::default();
    for y in y_start..(y_start + line_width) {
        for x in 0..image_params.width as i32 {
            pixels.insert((x, y));
        }
    }
    pixels
}

fn calc_up_edge_drawing_pixels(image_params: &ImageParams) -> Pixels {
    let center_x = (image_params.width / 2) as i32;
    let line_width = image_params.line_width as i32;
    let x_start = center_x - line_width / 2;
    let circle_center_y = (image_params.height / 2) as i32;
    let circle_outer_radius = image_params.circle_outer_radius as i32;

    let mut pixels = Pixels::default();
    for y in 0..(circle_center_y - circle_outer_radius) {
        for x in x_start..(x_start + line_width) {
            pixels.insert((x, y));
        }
    }
    pixels
}

fn calc_down_edge_drawing_pixels(image_params: &ImageParams) -> Pixels {
    let center_x = (image_params.width / 2) as i32;
    let line_width = image_params.line_width as i32;
    let x_start = center_x - line_width / 2;
    let circle_center_y = (image_params.height / 2) as i32;
    let circle_outer_radius = image_params.circle_outer_radius as i32;

    let mut pixels = Pixels::default();
    for y in (circle_center_y + circle_outer_radius + 1)..(image_params.height as i32) {
        for x in x_start..(x_start + line_width) {
            pixels.insert((x, y));
        }
    }
    pixels
}

fn calc_left_edge_drawing_pixels(image_params: &ImageParams) -> Pixels {
    let center_y = (image_params.height / 2) as i32;
    let line_width = image_params.line_width as i32;
    let y_start = center_y - line_width / 2;
    let circle_center_x = (image_params.width / 2) as i32;
    let circle_outer_radius = image_params.circle_outer_radius as i32;

    let mut pixels = Pixels::default();
    for y in y_start..(y_start + line_width) {
        for x in 0..(circle_center_x - circle_outer_radius) {
            pixels.insert((x, y));
        }
    }
    pixels
}

fn calc_right_edge_drawing_pixels(image_params: &ImageParams) -> Pixels {
    let center_y = (image_params.height / 2) as i32;
    let line_width = image_params.line_width as i32;
    let y_start = center_y - line_width / 2;
    let circle_center_x = (image_params.width / 2) as i32;
    let circle_outer_radius = image_params.circle_outer_radius as i32;

    let mut pixels = Pixels::default();
    for y in y_start..(y_start + line_width) {
        for x in (circle_center_x + circle_outer_radius + 1)..(image_params.width as i32) {
            pixels.insert((x, y));
        }
    }
    pixels
}

fn calc_right_top_edge_drawing_pixels(image_params: &ImageParams) -> Pixels {
    let (w, h, r) = (
        image_params.width as i32,
        image_params.height as i32,
        image_params.corner_radius() as i32,
    );
    let (x_offset, y_offset) = if w < h {
        (0, r - (h / 2))
    } else {
        ((w / 2) - r, 0)
    };
    calc_corner_edge_drawing_pixels(image_params, 0, h, x_offset, y_offset)
}

fn calc_left_top_edge_drawing_pixels(image_params: &ImageParams) -> Pixels {
    let (w, h, r) = (
        image_params.width as i32,
        image_params.height as i32,
        image_params.corner_radius() as i32,
    );
    let (x_offset, y_offset) = if w < h {
        (0, r - (h / 2))
    } else {
        (r - (w / 2), 0)
    };
    calc_corner_edge_drawing_pixels(image_params, w, h, x_offset, y_offset)
}

fn calc_right_bottom_edge_drawing_pixels(image_params: &ImageParams) -> Pixels {
    let (w, h, r) = (
        image_params.width as i32,
        image_params.height as i32,
        image_params.corner_radius() as i32,
    );
    let (x_offset, y_offset) = if w < h {
        (0, (h / 2) - r)
    } else {
        ((w / 2) - r, 0)
    };
    calc_corner_edge_drawing_pixels(image_params, 0, 0, x_offset, y_offset)
}

fn calc_left_bottom_edge_drawing_pixels(image_params: &ImageParams) -> Pixels {
    let (w, h, r) = (
        image_params.width as i32,
        image_params.height as i32,
        image_params.corner_radius() as i32,
    );
    let (x_offset, y_offset) = if w < h {
        (0, (h / 2) - r)
    } else {
        (r - (w / 2), 0)
    };
    calc_corner_edge_drawing_pixels(image_params, w, 0, x_offset, y_offset)
}

fn calc_corner_edge_drawing_pixels(
    image_params: &ImageParams,
    base_center_x: i32,
    base_center_y: i32,
    x_offset: i32,
    y_offset: i32,
) -> Pixels {
    // Bresenham's circle algorithm
    let curve_center_x = base_center_x;
    let curve_center_y = base_center_y;
    let line_width = image_params.line_width as i32;
    let half_line_width = line_width / 2;
    let adjust = if image_params.line_width.is_multiple_of(2) {
        0
    } else {
        1
    };
    let radius_base_length = image_params.corner_radius() as i32;
    let inner_radius = radius_base_length - half_line_width - adjust;
    let outer_radius = radius_base_length + half_line_width;

    let mut x = inner_radius;
    let mut y = 0;
    let mut p = 1 - inner_radius;

    let mut inner_pixels = Pixels::default();

    while x >= y {
        for dx in -x..=x {
            inner_pixels.insert((curve_center_x + dx, curve_center_y + y));
            inner_pixels.insert((curve_center_x + dx, curve_center_y - y));
        }
        for dx in -y..=y {
            inner_pixels.insert((curve_center_x + dx, curve_center_y + x));
            inner_pixels.insert((curve_center_x + dx, curve_center_y - x));
        }

        y += 1;
        if p <= 0 {
            p += 2 * y + 1;
        } else {
            x -= 1;
            p += 2 * y - 2 * x + 1;
        }
    }

    let mut x = outer_radius;
    let mut y = 0;
    let mut p = 1 - outer_radius;

    let mut outer_pixels = Pixels::default();

    while x >= y {
        for dx in -x..=x {
            outer_pixels.insert((curve_center_x + dx, curve_center_y + y));
            outer_pixels.insert((curve_center_x + dx, curve_center_y - y));
        }
        for dx in -y..=y {
            outer_pixels.insert((curve_center_x + dx, curve_center_y + x));
            outer_pixels.insert((curve_center_x + dx, curve_center_y - x));
        }

        y += 1;
        if p <= 0 {
            p += 2 * y + 1;
        } else {
            x -= 1;
            p += 2 * y - 2 * x + 1;
        }
    }

    let mut pixels: Pixels = outer_pixels
        .difference(&inner_pixels)
        .filter(|p| {
            p.0 >= 0
                && p.0 < image_params.width as i32
                && p.1 >= 0
                && p.1 < image_params.height as i32
        })
        .map(|p| (p.0 + x_offset, p.1 + y_offset))
        .collect();

    if image_params.width < image_params.height {
        let (ys, ye) = if y_offset < 0 {
            (base_center_y + y_offset, base_center_y)
        } else {
            (base_center_y, base_center_y + y_offset)
        };
        let center_x = (image_params.width / 2) as i32;
        let x_start = center_x - line_width / 2;
        for x in x_start..(x_start + line_width) {
            for y in ys..ye {
                pixels.insert((x, y));
            }
        }
    }
    if image_params.width > image_params.height {
        let (xs, xe) = if x_offset < 0 {
            (base_center_x + x_offset, base_center_x)
        } else {
            (base_center_x, base_center_x + x_offset)
        };
        let center_y = (image_params.height / 2) as i32;
        let y_start = center_y - line_width / 2;
        for y in y_start..(y_start + line_width) {
            for x in xs..xe {
                pixels.insert((x, y));
            }
        }
    }

    pixels
}

pub fn calc_graph_row_image(
    commit_pos_x: usize,
    cell_count: usize,
    edges: &[Edge],
    image_params: &ImageParams,
    drawing_pixels: &DrawingPixels,
    graph_style: GraphStyle,
) -> GraphRowImage {
    let image_width = (image_params.width as usize * cell_count) as u32;
    let image_height = image_params.height as u32;

    let mut img_buf = image::ImageBuffer::new(image_width, image_height);

    draw_background(&mut img_buf, image_params);
    draw_commit_circle(&mut img_buf, commit_pos_x, image_params, drawing_pixels);

    match graph_style {
        GraphStyle::Rounded => {
            for edge in edges {
                draw_edge(&mut img_buf, edge, image_params, drawing_pixels)
            }
        }
        GraphStyle::Angular => {
            let (vertial_edges, horizontal_edges): (Vec<&Edge>, Vec<&Edge>) = edges
                .iter()
                .partition(|e| e.edge_type.is_vertically_related());
            for edge in vertial_edges {
                draw_edge(&mut img_buf, edge, image_params, drawing_pixels)
            }
            let mut horizontal_edges_map: FxHashMap<usize, Vec<&Edge>> = FxHashMap::default();
            for edge in horizontal_edges {
                horizontal_edges_map
                    .entry(edge.associated_line_pos_x)
                    .or_default()
                    .push(edge);
            }
            for edges in horizontal_edges_map.values() {
                draw_diagonal_connected_edge(&mut img_buf, edges, image_params);
            }
        }
    }

    let bytes = build_image(&img_buf, image_width, image_height);

    GraphRowImage { bytes, cell_count }
}

fn draw_background(
    img_buf: &mut image::ImageBuffer<image::Rgba<u8>, Vec<u8>>,
    image_params: &ImageParams,
) {
    if image_params.background_color[3] == 0 {
        // If the alpha value is 0, the background is transparent, so we don't need to draw it.
        return;
    }
    for pixel in img_buf.pixels_mut() {
        *pixel = image_params.background_color;
    }
}

fn draw_commit_circle(
    img_buf: &mut image::ImageBuffer<image::Rgba<u8>, Vec<u8>>,
    circle_pos_x: usize,
    image_params: &ImageParams,
    drawing_pixels: &DrawingPixels,
) {
    let x_offset = (circle_pos_x * image_params.width as usize) as i32;
    let color = image_params.edge_color(circle_pos_x);

    for (x, y) in &drawing_pixels.circle {
        let x = (*x + x_offset) as u32;
        let y = *y as u32;

        let pixel = img_buf.get_pixel_mut(x, y);
        *pixel = color;
    }

    if image_params.circle_edge_color[3] == 0 {
        // If the alpha value is 0, the circle edge is transparent, so we don't need to draw it.
        return;
    }

    for (x, y) in &drawing_pixels.circle_edge {
        let x = (*x + x_offset) as u32;
        let y = *y as u32;

        let pixel = img_buf.get_pixel_mut(x, y);
        *pixel = image_params.circle_edge_color;
    }
}

fn draw_edge(
    img_buf: &mut image::ImageBuffer<image::Rgba<u8>, Vec<u8>>,
    edge: &Edge,
    image_params: &ImageParams,
    drawing_pixels: &DrawingPixels,
) {
    let pixels = match edge.edge_type {
        EdgeType::Vertical => &drawing_pixels.vertical_edge,
        EdgeType::Horizontal => &drawing_pixels.horizontal_edge,
        EdgeType::Up => &drawing_pixels.up_edge,
        EdgeType::Down => &drawing_pixels.down_edge,
        EdgeType::Left => &drawing_pixels.left_edge,
        EdgeType::Right => &drawing_pixels.right_edge,
        EdgeType::RightTop => &drawing_pixels.right_top_edge,
        EdgeType::RightBottom => &drawing_pixels.right_bottom_edge,
        EdgeType::LeftTop => &drawing_pixels.left_top_edge,
        EdgeType::LeftBottom => &drawing_pixels.left_bottom_edge,
    };

    let x_offset = (edge.pos_x * image_params.width as usize) as i32;
    let color = image_params.edge_color(edge.associated_line_pos_x);

    for (x, y) in pixels {
        let x = (*x + x_offset) as u32;
        let y = *y as u32;

        let pixel = img_buf.get_pixel_mut(x, y);
        *pixel = color;
    }
}

// fixme: cache edge drawing range calculations
fn draw_diagonal_connected_edge(
    img_buf: &mut image::ImageBuffer<image::Rgba<u8>, Vec<u8>>,
    edges: &[&Edge],
    image_params: &ImageParams,
) {
    let corner_edges = edges.iter().filter(|e| {
        matches!(
            e.edge_type,
            EdgeType::RightBottom | EdgeType::LeftBottom | EdgeType::RightTop | EdgeType::LeftTop
        )
    });

    for corner_edge in corner_edges {
        let expected_side_edge_type = match corner_edge.edge_type {
            EdgeType::RightBottom | EdgeType::RightTop => EdgeType::Right,
            EdgeType::LeftBottom | EdgeType::LeftTop => EdgeType::Left,
            _ => unreachable!("unexpected edge type for corner edge"),
        };
        let side_edge_opt = edges
            .iter()
            .find(|e| e.edge_type == expected_side_edge_type);
        // No side edge found, nothing to draw (should not happen)
        if let Some(side_edge) = side_edge_opt {
            let line_width_f64 = image_params.line_width as f64;
            let line_width_i32 = image_params.line_width as i32;

            // NOTE: Select y_offset of the corner edge based on the cell width.
            // The hard-coded value `height / 10.0` is based on the assumption that the cell
            // has a 1:1 aspect ratio, and does not work well for non-1:1 ratios.
            let y_offset = if image_params.width == image_params.height {
                image_params.height as f64 / 10.0
            } else {
                image_params.height as f64 / 2.0 - image_params.corner_radius() as f64
            };

            match corner_edge.edge_type {
                EdgeType::RightBottom | EdgeType::LeftBottom => {
                    let start_pos_center = Point::new(
                        (side_edge.pos_x * image_params.width as usize) as f64
                            + (image_params.width as f64 / 2.0),
                        image_params.height as f64 / 2.0,
                    );
                    let end_pos_center = Point::new(
                        (corner_edge.pos_x * image_params.width as usize) as f64
                            + (image_params.width as f64 / 2.0),
                        y_offset,
                    );

                    let line_vec = end_pos_center - start_pos_center;
                    let unit_vec = line_vec.normalize();
                    let normal_vec = unit_vec.perpendicular();

                    let line_start =
                        start_pos_center + unit_vec * (image_params.circle_outer_radius as f64);
                    let line_start_1 = line_start + normal_vec * (line_width_f64 / 2.0);
                    let line_start_2 = line_start - normal_vec * (line_width_f64 / 2.0);

                    let half_width = line_width_f64 / 2.0;
                    let slope = unit_vec.y / unit_vec.x;

                    let vertical_left_x = end_pos_center.x - half_width;
                    let vertical_right_x = end_pos_center.x + half_width;

                    let corner_1 = Point::new(
                        vertical_right_x,
                        line_start_1.y + slope * (vertical_right_x - line_start_1.x),
                    );
                    let corner_2 = Point::new(
                        vertical_left_x,
                        line_start_2.y + slope * (vertical_left_x - line_start_2.x),
                    );

                    let vertices = [line_start_1, corner_1, corner_2, line_start_2];

                    let (min_x, min_y, max_x, max_y) = bounding_box_u32(&vertices);
                    for y in min_y..max_y {
                        for x in min_x..max_x {
                            if x < img_buf.width() && y < img_buf.height() {
                                let p = Point::new(x as f64 + 0.5, y as f64 + 0.5);

                                if p.is_inside_polygon(&vertices) {
                                    let pixel = img_buf.get_pixel_mut(x, y);
                                    let color =
                                        image_params.edge_color(side_edge.associated_line_pos_x);
                                    *pixel = color;
                                }
                            }
                        }
                    }

                    let y_end = corner_1.y.max(corner_2.y) as u32;
                    let end_center_x_i32 = end_pos_center.x as i32;
                    let x_start = end_center_x_i32 - line_width_i32 / 2;
                    for y in 0..y_end {
                        for i in 0..line_width_i32 {
                            let x = (x_start + i) as u32;
                            if x < img_buf.width() && y < img_buf.height() {
                                let pixel = img_buf.get_pixel_mut(x, y);
                                let color =
                                    image_params.edge_color(side_edge.associated_line_pos_x);
                                *pixel = color;
                            }
                        }
                    }
                }
                EdgeType::RightTop | EdgeType::LeftTop => {
                    let start_pos_center = Point::new(
                        (side_edge.pos_x * image_params.width as usize) as f64
                            + (image_params.width as f64 / 2.0),
                        image_params.height as f64 / 2.0,
                    );
                    let end_pos_center = Point::new(
                        (corner_edge.pos_x * image_params.width as usize) as f64
                            + (image_params.width as f64 / 2.0),
                        image_params.height as f64 - y_offset,
                    );

                    let line_vec = end_pos_center - start_pos_center;
                    let unit_vec = line_vec.normalize();
                    let normal_vec = unit_vec.perpendicular();

                    let line_start =
                        start_pos_center + unit_vec * (image_params.circle_outer_radius as f64);
                    let line_start_1 = line_start + normal_vec * (line_width_f64 / 2.0);
                    let line_start_2 = line_start - normal_vec * (line_width_f64 / 2.0);

                    let half_width = line_width_f64 / 2.0;
                    let slope = unit_vec.y / unit_vec.x;

                    let vertical_left_x = end_pos_center.x - half_width;
                    let vertical_right_x = end_pos_center.x + half_width;

                    let corner_1 = Point::new(
                        vertical_left_x,
                        line_start_1.y + slope * (vertical_left_x - line_start_1.x),
                    );
                    let corner_2 = Point::new(
                        vertical_right_x,
                        line_start_2.y + slope * (vertical_right_x - line_start_2.x),
                    );

                    let vertices = [line_start_1, corner_1, corner_2, line_start_2];

                    let (min_x, min_y, max_x, max_y) = bounding_box_u32(&vertices);
                    for y in min_y..max_y {
                        for x in min_x..max_x {
                            if x < img_buf.width() && y < img_buf.height() {
                                let p = Point::new(x as f64 + 0.5, y as f64 + 0.5);

                                if p.is_inside_polygon(&vertices) {
                                    let pixel = img_buf.get_pixel_mut(x, y);
                                    let color =
                                        image_params.edge_color(side_edge.associated_line_pos_x);
                                    *pixel = color;
                                }
                            }
                        }
                    }

                    let y_start = corner_1.y.min(corner_2.y) as u32;
                    let end_center_x_i32 = end_pos_center.x as i32;
                    let x_start = end_center_x_i32 - line_width_i32 / 2;
                    for y in (y_start + 1)..image_params.height as u32 {
                        for i in 0..line_width_i32 {
                            let x = (x_start + i) as u32;
                            if x < img_buf.width() && y < img_buf.height() {
                                let pixel = img_buf.get_pixel_mut(x, y);
                                let color =
                                    image_params.edge_color(side_edge.associated_line_pos_x);
                                *pixel = color;
                            }
                        }
                    }
                }
                _ => unreachable!("unexpected edge type for corner edge"),
            }
        }
    }
}

fn build_image(img_buf: &[u8], image_width: u32, image_height: u32) -> Vec<u8> {
    let mut bytes = Cursor::new(Vec::new());
    image::write_buffer_with_format(
        &mut bytes,
        img_buf,
        image_width,
        image_height,
        image::ColorType::Rgba8,
        image::ImageFormat::Png,
    )
    .unwrap();
    bytes.into_inner()
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use image::GenericImage;
    use rstest::rstest;

    use crate::{config::GraphColorConfig, git::Commit};

    use super::*;
    use EdgeType::*;

    const OUTPUT_DIR: &str = "./out/ut/graph/image";

    type TestParam = (usize, Vec<(EdgeType, usize, usize)>);

    // Note: The output contents are not verified by the code.

    #[rstest]
    #[case("default_params_rounded", GraphStyle::Rounded)]
    #[case("default_params_angular", GraphStyle::Angular)]
    fn test_calc_graph_row_image_default_params(
        #[case] file_name: &str,
        #[case] graph_style: GraphStyle,
    ) {
        let params = simple_test_params();
        let cell_count = 4;
        let graph_color_config = GraphColorConfig::default();
        let graph_color_set = GraphColorSet::new(&graph_color_config);
        let cell_width_type = CellWidthType::Double;
        let image_params = ImageParams::new(&graph_color_set, cell_width_type);
        let drawing_pixels = DrawingPixels::new(&image_params);

        test_calc_graph_row_image(
            params,
            cell_count,
            image_params,
            drawing_pixels,
            graph_style,
            file_name,
        );
    }

    #[rstest]
    #[case("wide_image_rounded", GraphStyle::Rounded)]
    #[case("wide_image_angular", GraphStyle::Angular)]
    fn test_calc_graph_row_image_wide_image(
        #[case] file_name: &str,
        #[case] graph_style: GraphStyle,
    ) {
        let params = simple_test_params();
        let cell_count = 4;
        let graph_color_config = GraphColorConfig::default();
        let graph_color_set = GraphColorSet::new(&graph_color_config);
        let cell_width_type = CellWidthType::Double;
        let mut image_params = ImageParams::new(&graph_color_set, cell_width_type);
        image_params.width = 100;
        let drawing_pixels = DrawingPixels::new(&image_params);

        test_calc_graph_row_image(
            params,
            cell_count,
            image_params,
            drawing_pixels,
            graph_style,
            file_name,
        );
    }

    #[rstest]
    #[case("tall_image_rounded", GraphStyle::Rounded)]
    #[case("tall_image_angular", GraphStyle::Angular)]
    fn test_calc_graph_row_image_tall_image(
        #[case] file_name: &str,
        #[case] graph_style: GraphStyle,
    ) {
        let params = simple_test_params();
        let cell_count = 4;
        let graph_color_config = GraphColorConfig::default();
        let graph_color_set = GraphColorSet::new(&graph_color_config);
        let cell_width_type = CellWidthType::Double;
        let mut image_params = ImageParams::new(&graph_color_set, cell_width_type);
        image_params.height = 100;
        let drawing_pixels = DrawingPixels::new(&image_params);

        test_calc_graph_row_image(
            params,
            cell_count,
            image_params,
            drawing_pixels,
            graph_style,
            file_name,
        );
    }

    #[rstest]
    #[case("single_cell_width_rounded", GraphStyle::Rounded)]
    #[case("single_cell_width_angular", GraphStyle::Angular)]
    fn test_calc_graph_row_image_single_cell_width(
        #[case] file_name: &str,
        #[case] graph_style: GraphStyle,
    ) {
        let params = simple_test_params();
        let cell_count = 4;
        let graph_color_config = GraphColorConfig::default();
        let graph_color_set = GraphColorSet::new(&graph_color_config);
        let cell_width_type = CellWidthType::Single;
        let image_params = ImageParams::new(&graph_color_set, cell_width_type);
        let drawing_pixels = DrawingPixels::new(&image_params);

        test_calc_graph_row_image(
            params,
            cell_count,
            image_params,
            drawing_pixels,
            graph_style,
            file_name,
        );
    }

    #[rstest]
    #[case("circle_radius_rounded", GraphStyle::Rounded)]
    #[case("circle_radius_angular", GraphStyle::Angular)]
    fn test_calc_graph_row_image_circle_radius(
        #[case] file_name: &str,
        #[case] graph_style: GraphStyle,
    ) {
        let params = straight_test_params();
        let cell_count = 2;
        let graph_color_config = GraphColorConfig::default();
        let graph_color_set = GraphColorSet::new(&graph_color_config);
        let cell_width_type = CellWidthType::Double;
        let mut image_params = ImageParams::new(&graph_color_set, cell_width_type);
        image_params.circle_inner_radius = 5;
        image_params.circle_outer_radius = 12;
        let drawing_pixels = DrawingPixels::new(&image_params);

        test_calc_graph_row_image(
            params,
            cell_count,
            image_params,
            drawing_pixels,
            graph_style,
            file_name,
        );
    }

    #[rstest]
    #[case("line_width_rounded", GraphStyle::Rounded)]
    #[case("line_width_angular", GraphStyle::Angular)]
    fn test_calc_graph_row_image_line_width(
        #[case] file_name: &str,
        #[case] graph_style: GraphStyle,
    ) {
        let params = straight_test_params();
        let cell_count = 2;
        let graph_color_config = GraphColorConfig::default();
        let graph_color_set = GraphColorSet::new(&graph_color_config);
        let cell_width_type = CellWidthType::Double;
        let mut image_params = ImageParams::new(&graph_color_set, cell_width_type);
        image_params.line_width = 1;
        let drawing_pixels = DrawingPixels::new(&image_params);

        test_calc_graph_row_image(
            params,
            cell_count,
            image_params,
            drawing_pixels,
            graph_style,
            file_name,
        );
    }

    #[rstest]
    #[case("color_rounded", GraphStyle::Rounded)]
    #[case("color_angular", GraphStyle::Angular)]
    fn test_calc_graph_row_image_color(#[case] file_name: &str, #[case] graph_style: GraphStyle) {
        let params = branches_test_params();
        let cell_count = 7;
        let graph_color_config = GraphColorConfig {
            branches: vec![
                "#c8c864".into(),
                "#64c8c8".into(),
                "#646464".into(),
                "#c864c8".into(),
            ],
            edge: "#ffffff".into(),
            background: "#00ff0070".into(),
        };
        let graph_color_set = GraphColorSet::new(&graph_color_config);
        let cell_width_type = CellWidthType::Double;
        let image_params = ImageParams::new(&graph_color_set, cell_width_type);
        let drawing_pixels = DrawingPixels::new(&image_params);

        test_calc_graph_row_image(
            params,
            cell_count,
            image_params,
            drawing_pixels,
            graph_style,
            file_name,
        );
    }

    #[rustfmt::skip]
    fn simple_test_params() -> Vec<TestParam> {
        vec![
            (1, vec![(LeftBottom, 0, 0), (Left, 1, 0), (Down, 1, 1), (Right, 1, 3), (Horizontal, 2, 3), (RightBottom, 3, 3)]),
            (3, vec![(Vertical, 0, 0), (Up, 3, 3), (Down, 3, 3)]),
            (2, vec![(LeftTop, 0, 0), (Horizontal, 1, 0), (Left, 2, 0), (Up, 2, 2), (Right, 2, 3), (RightTop, 3, 3)]),
        ]
    }

    #[rustfmt::skip]
    fn straight_test_params() -> Vec<TestParam> {
        vec![
            (0, vec![(Up, 0, 0), (Down, 0, 0)]),
            (0, vec![(Up, 0, 0), (Down, 0, 0), (Right, 0, 1), (RightBottom, 1, 1)]),
            (1, vec![(Vertical, 0, 0), (Up, 1, 1), (Down, 1, 1)]),
            (0, vec![(Up, 0, 0), (Down, 0, 0), (Right, 0, 1), (RightTop, 1, 1)]),
        ]
    }

    #[rustfmt::skip]
    fn branches_test_params() -> Vec<TestParam> {
        vec![
            (0, vec![(Up, 0, 0), (Down, 0, 0),
                    (Right, 0, 1), (RightBottom, 1, 1),
                    (Right, 0, 2), (Horizontal, 1, 2), (RightBottom, 2, 2),
                    (Right, 0, 3), (Horizontal, 1, 3), (Horizontal, 2, 3), (RightBottom, 3, 3),
                    (Right, 0, 4), (Horizontal, 1, 4), (Horizontal, 2, 4), (Horizontal, 3, 4), (RightBottom, 4, 4),
                    (Right, 0, 5), (Horizontal, 1, 5), (Horizontal, 2, 5), (Horizontal, 3, 5), (Horizontal, 4, 5), (RightBottom, 5, 5),
                    (Right, 0, 6), (Horizontal, 1, 6), (Horizontal, 2, 6), (Horizontal, 3, 6), (Horizontal, 4, 6), (Horizontal, 5, 6), (RightBottom, 6, 6)]),
            (6, vec![(Vertical, 0, 0), (Vertical, 1, 1), (Vertical, 2, 2), (Vertical, 3, 3), (Vertical, 4, 4), (Vertical, 5, 5), (Down, 6, 6), (Up, 6, 6)]),
        ]
    }

    fn test_calc_graph_row_image(
        params: Vec<TestParam>,
        cell_count: usize,
        image_params: ImageParams,
        drawing_pixels: DrawingPixels,
        graph_style: GraphStyle,
        file_name: &str,
    ) {
        let graph_row_images: Vec<GraphRowImage> = params
            .into_iter()
            .map(|(commit_pos_x, edges)| {
                let edges: Vec<Edge> = edges
                    .into_iter()
                    .map(|t| Edge::new(t.0, t.1, t.2))
                    .collect();
                calc_graph_row_image(
                    commit_pos_x,
                    cell_count,
                    &edges,
                    &image_params,
                    &drawing_pixels,
                    graph_style,
                )
            })
            .collect();

        save_image(&graph_row_images, &image_params, cell_count, file_name);
    }

    fn save_image(
        graph_row_images: &[GraphRowImage],
        image_params: &ImageParams,
        cell_count: usize,
        file_name: &str,
    ) {
        let rows_len = graph_row_images.len() as u32;
        let image_width = image_params.width as u32 * cell_count as u32;
        let image_height = image_params.height as u32 * rows_len;

        let mut img_buf: image::ImageBuffer<image::Rgba<u8>, Vec<u8>> =
            image::ImageBuffer::new(image_width, image_height);

        for (i, graph_row_image) in graph_row_images.iter().enumerate() {
            let image = image::load_from_memory(&graph_row_image.bytes).unwrap();
            let y = image_params.height as u32 * (rows_len - (i as u32) - 1);
            img_buf.copy_from(&image, 0, y).unwrap();

            for x in 0..cell_count {
                let x_offset = x as u32 * image_params.width as u32;
                let y_offset = y;
                draw_border(&mut img_buf, image_params, x_offset, y_offset);
            }
        }

        create_output_dirs(OUTPUT_DIR);
        let file_name = format!("{OUTPUT_DIR}/{file_name}.png");
        image::save_buffer(
            file_name,
            &img_buf,
            image_width,
            image_height,
            image::ColorType::Rgba8,
        )
        .unwrap();
    }

    fn draw_border(
        img_buf: &mut image::ImageBuffer<image::Rgba<u8>, Vec<u8>>,
        image_params: &ImageParams,
        x_offset: u32,
        y_offset: u32,
    ) {
        for x in 0..image_params.width {
            for y in 0..image_params.height {
                if x == 0 || x == image_params.width - 1 || y == 0 || y == image_params.height - 1 {
                    img_buf.put_pixel(
                        x as u32 + x_offset,
                        y as u32 + y_offset,
                        image::Rgba([255, 0, 0, 50]),
                    );
                }
            }
        }
    }

    fn create_output_dirs(path: &str) {
        let path = Path::new(path);
        std::fs::create_dir_all(path).unwrap();
    }

    // ------------------------------------------------------------------
    // ASCII renderer
    // ------------------------------------------------------------------

    fn ascii_test_image_params(cell_width_type: CellWidthType) -> ImageParams {
        let graph_color_config = GraphColorConfig::default();
        let graph_color_set = GraphColorSet::new(&graph_color_config);
        ImageParams::new(&graph_color_set, cell_width_type)
    }

    fn render_test_ascii_row(
        pos_x: usize,
        cell_count: usize,
        edges: &[(EdgeType, usize, usize)],
        graph_style: GraphStyle,
        cell_width_type: CellWidthType,
    ) -> PreparedImage {
        let edges: Vec<Edge> = edges
            .iter()
            .map(|&(edge_type, x, line_pos_x)| Edge::new(edge_type, x, line_pos_x))
            .collect();
        let image_params = ascii_test_image_params(cell_width_type);
        render_ascii_row(
            pos_x,
            cell_count,
            &edges,
            &image_params,
            graph_style,
            cell_width_type,
        )
    }

    fn symbols(image: &PreparedImage) -> String {
        image.cells().iter().map(|c| c.symbol()).collect()
    }

    #[rstest]
    #[case(Vertical, (true, true, false, false))]
    #[case(Horizontal, (false, false, true, true))]
    #[case(Up, (true, false, false, false))]
    #[case(Down, (false, true, false, false))]
    #[case(Left, (false, false, true, false))]
    #[case(Right, (false, false, false, true))]
    #[case(LeftTop, (false, true, false, true))]
    #[case(RightTop, (false, true, true, false))]
    #[case(LeftBottom, (true, false, false, true))]
    #[case(RightBottom, (true, false, true, false))]
    fn test_edge_directions(
        #[case] edge_type: EdgeType,
        #[case] expected: (bool, bool, bool, bool),
    ) {
        let d = edge_directions(edge_type);
        assert_eq!((d.up, d.down, d.left, d.right), expected);
    }

    fn directions(up: bool, down: bool, left: bool, right: bool) -> AsciiDirections {
        AsciiDirections {
            up,
            down,
            left,
            right,
        }
    }

    #[rstest]
    // T-junctions and the cross are the same in both styles.
    #[case(directions(true, true, true, true), GraphStyle::Rounded, '┼')]
    #[case(directions(true, true, true, false), GraphStyle::Rounded, '┤')]
    #[case(directions(true, true, false, true), GraphStyle::Rounded, '├')]
    #[case(directions(true, false, true, true), GraphStyle::Rounded, '┴')]
    #[case(directions(false, true, true, true), GraphStyle::Rounded, '┬')]
    // Straight lines.
    #[case(directions(true, true, false, false), GraphStyle::Rounded, '│')]
    #[case(directions(false, false, true, true), GraphStyle::Rounded, '─')]
    // Corners differ between the styles.
    #[case(directions(true, false, false, true), GraphStyle::Rounded, '╰')]
    #[case(directions(true, false, true, false), GraphStyle::Rounded, '╯')]
    #[case(directions(false, true, false, true), GraphStyle::Rounded, '╭')]
    #[case(directions(false, true, true, false), GraphStyle::Rounded, '╮')]
    #[case(directions(true, false, false, true), GraphStyle::Angular, '└')]
    #[case(directions(true, false, true, false), GraphStyle::Angular, '┘')]
    #[case(directions(false, true, false, true), GraphStyle::Angular, '┌')]
    #[case(directions(false, true, true, false), GraphStyle::Angular, '┐')]
    // A line that terminates in the cell is drawn as a full vertical or horizontal line.
    #[case(directions(true, false, false, false), GraphStyle::Rounded, '│')]
    #[case(directions(false, true, false, false), GraphStyle::Rounded, '│')]
    #[case(directions(false, false, true, false), GraphStyle::Rounded, '─')]
    #[case(directions(false, false, false, true), GraphStyle::Rounded, '─')]
    // An empty cell is blank.
    #[case(directions(false, false, false, false), GraphStyle::Rounded, ' ')]
    fn test_ascii_symbol(
        #[case] d: AsciiDirections,
        #[case] graph_style: GraphStyle,
        #[case] expected: char,
    ) {
        assert_eq!(ascii_symbol(d, graph_style), expected);
    }

    #[test]
    fn test_render_ascii_row_single_width_straight() {
        // A straight graph: the commit is drawn at its own column, the trunk passes through the
        // other ones.
        let image = render_test_ascii_row(
            3,
            4,
            &[(Vertical, 0, 0), (Up, 3, 3), (Down, 3, 3)],
            GraphStyle::Rounded,
            CellWidthType::Single,
        );
        assert_eq!(symbols(&image), "│  ●");
    }

    #[test]
    fn test_render_ascii_row_single_width_branch_source() {
        // A child branches off the commit at pos_x=1 towards the column 3.
        let image = render_test_ascii_row(
            1,
            4,
            &[
                (LeftBottom, 0, 0),
                (Left, 1, 0),
                (Down, 1, 1),
                (Right, 1, 3),
                (Horizontal, 2, 3),
                (RightBottom, 3, 3),
            ],
            GraphStyle::Rounded,
            CellWidthType::Single,
        );
        assert_eq!(symbols(&image), "╰●─╯");
    }

    #[test]
    fn test_render_ascii_row_single_width_merge() {
        // A merge commit at pos_x=2 that merged the parent of the column 0: the merge commit is
        // an open circle, and the corners of the merge show up as ╭ and ╮.
        let image = render_test_ascii_row(
            2,
            4,
            &[
                (LeftTop, 0, 0),
                (Horizontal, 1, 0),
                (Left, 2, 0),
                (Up, 2, 2),
                (Right, 2, 3),
                (RightTop, 3, 3),
            ],
            GraphStyle::Rounded,
            CellWidthType::Single,
        );
        assert_eq!(symbols(&image), "╭─○╮");
    }

    #[test]
    fn test_render_ascii_row_single_width_angular() {
        // The same merge row with the angular style.
        let image = render_test_ascii_row(
            2,
            4,
            &[
                (LeftTop, 0, 0),
                (Horizontal, 1, 0),
                (Left, 2, 0),
                (Up, 2, 2),
                (Right, 2, 3),
                (RightTop, 3, 3),
            ],
            GraphStyle::Angular,
            CellWidthType::Single,
        );
        assert_eq!(symbols(&image), "┌─○┐");
    }

    #[test]
    fn test_render_ascii_row_double_width_branch_source() {
        // In double width the cells of a column are followed by a filler cell, which carries the
        // horizontal lines across the gap between the columns. There is no arrow here, since the
        // commit is branched off rather than merged into.
        let image = render_test_ascii_row(
            1,
            4,
            &[
                (LeftBottom, 0, 0),
                (Left, 1, 0),
                (Down, 1, 1),
                (Right, 1, 3),
                (Horizontal, 2, 3),
                (RightBottom, 3, 3),
            ],
            GraphStyle::Rounded,
            CellWidthType::Double,
        );
        assert_eq!(symbols(&image), "╰─●───╯ ");
    }

    #[test]
    fn test_render_ascii_row_double_width_merge_from_left() {
        // The branch of the parent at the column 0 merges into the commit at pos_x=2, so the
        // arrow points right into the open circle.
        let image = render_test_ascii_row(
            2,
            3,
            &[
                (LeftTop, 0, 0),
                (Horizontal, 1, 0),
                (Left, 2, 0),
                (Up, 2, 2),
                (Down, 2, 2),
            ],
            GraphStyle::Rounded,
            CellWidthType::Double,
        );
        assert_eq!(symbols(&image), "╭──>○ ");
    }

    #[test]
    fn test_render_ascii_row_double_width_merge_from_right() {
        // The branch of the parent at the column 2 merges into the commit at pos_x=0, so the
        // arrow points left into the open circle.
        let image = render_test_ascii_row(
            0,
            3,
            &[
                (Right, 0, 2),
                (Horizontal, 1, 2),
                (RightTop, 2, 2),
                (Up, 0, 0),
                (Down, 0, 0),
            ],
            GraphStyle::Rounded,
            CellWidthType::Double,
        );
        assert_eq!(symbols(&image), "○<──╮ ");
    }

    #[test]
    fn test_render_ascii_row_double_width_merge_from_both_sides() {
        // A commit that merged two branches: the arrows point into the open circle from both
        // sides, and the branches continue on the corners of their own columns.
        let image = render_test_ascii_row(
            2,
            4,
            &[
                (LeftTop, 0, 0),
                (Horizontal, 1, 0),
                (Left, 2, 0),
                (Up, 2, 2),
                (Right, 2, 3),
                (RightTop, 3, 3),
            ],
            GraphStyle::Rounded,
            CellWidthType::Double,
        );
        assert_eq!(symbols(&image), "╭──>○<╮ ");
    }

    #[rstest]
    // Cells that are shared by more than one edge are drawn as a single junction character.
    #[case(vec![(Vertical, 0, 0), (Vertical, 0, 0)], "│  ●")]
    #[case(vec![(Vertical, 0, 0), (Horizontal, 0, 0)], "┼  ●")]
    #[case(vec![(Vertical, 0, 0), (Right, 0, 0)], "├  ●")]
    #[case(vec![(Vertical, 0, 0), (Left, 0, 0)], "┤  ●")]
    #[case(vec![(Horizontal, 0, 0), (Down, 0, 0)], "┬  ●")]
    #[case(vec![(Horizontal, 0, 0), (Up, 0, 0)], "┴  ●")]
    #[case(vec![(Horizontal, 0, 0), (LeftBottom, 0, 0)], "┴  ●")]
    // A corner of a merge is composed as well, and makes the whole row a merge row.
    #[case(vec![(Vertical, 0, 0), (RightTop, 0, 0)], "┤  ○")]
    fn test_render_ascii_row_composes_the_edges_of_a_cell(
        #[case] cell_edges: Vec<(EdgeType, usize, usize)>,
        #[case] expected: &str,
    ) {
        let mut edges = cell_edges;
        edges.push((Up, 3, 3)); // the commit cell
        let image = render_test_ascii_row(3, 4, &edges, GraphStyle::Rounded, CellWidthType::Single);
        assert_eq!(symbols(&image), expected);
    }

    #[test]
    fn test_render_ascii_row_prefers_the_color_of_a_vertical_edge() {
        // A cell that is shared by a vertical trunk and a horizontal branch uses the color of
        // the vertical line, which is the branch the eye follows through the cell.
        let image = render_test_ascii_row(
            2,
            3,
            &[(Vertical, 0, 0), (Horizontal, 0, 1), (Up, 2, 2)],
            GraphStyle::Rounded,
            CellWidthType::Single,
        );
        assert_eq!(symbols(&image), "┼ ●");
        assert_eq!(
            image.cells()[0].style().fg,
            Some(Color::Rgb(224, 108, 118)) // branch color 0
        );

        // Without a vertical line, the color of the horizontal line is used.
        let image = render_test_ascii_row(
            2,
            3,
            &[(Horizontal, 0, 1), (Up, 2, 2)],
            GraphStyle::Rounded,
            CellWidthType::Single,
        );
        assert_eq!(symbols(&image), "─ ●");
        assert_eq!(
            image.cells()[0].style().fg,
            Some(Color::Rgb(152, 195, 121)) // branch color 1
        );
    }

    #[test]
    fn test_render_ascii_row_cell_count_and_no_upload_data() {
        let edges = [(Vertical, 0, 0), (Up, 1, 1), (Down, 1, 1)];

        let single =
            render_test_ascii_row(1, 2, &edges, GraphStyle::Rounded, CellWidthType::Single);
        assert_eq!(single.cells().len(), 2);
        assert!(single.cells().iter().all(|c| !c.skip()));

        let mut double =
            render_test_ascii_row(1, 2, &edges, GraphStyle::Rounded, CellWidthType::Double);
        assert_eq!(double.cells().len(), 4);
        assert_eq!(symbols(&double), "│ ● ");
        assert!(double.take_upload_data().is_none());
    }

    #[test]
    fn test_graph_image_manager_with_the_ascii_protocol() {
        let commits: Vec<Commit> = ["1", "2"]
            .iter()
            .enumerate()
            .map(|(i, subject)| Commit {
                commit_hash: CommitHash::from(format!("{:040x}", i + 1).as_str()),
                subject: (*subject).into(),
                ..Commit::default()
            })
            .collect();

        let mut commit_pos_map = FxHashMap::default();
        commit_pos_map.insert(&commits[0].commit_hash, (0, 0));
        commit_pos_map.insert(&commits[1].commit_hash, (1, 1));

        let graph = Graph {
            commits: commits.iter().collect(),
            commit_pos_map,
            edges: vec![
                // a child branches off the first commit to the second column
                vec![
                    Edge::new(Up, 0, 0),
                    Edge::new(Right, 0, 1),
                    Edge::new(RightBottom, 1, 1),
                ],
                vec![
                    Edge::new(Vertical, 0, 0),
                    Edge::new(Up, 1, 1),
                    Edge::new(Down, 1, 1),
                ],
            ],
            max_pos_x: 1,
            warning: None,
        };

        let graph_color_config = GraphColorConfig::default();
        let graph_color_set = GraphColorSet::new(&graph_color_config);
        let mut manager = GraphImageManager::new(
            &graph,
            &graph_color_set,
            CellWidthType::Single,
            GraphStyle::Rounded,
            GraphImageWidthMode::Compact,
            ImageProtocol::Ascii,
        );

        for commit in &commits {
            manager.ensure_uploaded(&commit.commit_hash);
        }

        // The ASCII graph is not uploaded to the terminal, so there is nothing to clean up.
        assert!(manager.drain_pending_uploads().is_empty());
        assert!(manager.image_ids().is_empty());
        assert_eq!(manager.graph_cell_width(), 2);

        assert_eq!(
            symbols(manager.prepared_image(&commits[0].commit_hash)),
            "●╯"
        );
        assert_eq!(
            symbols(manager.prepared_image(&commits[1].commit_hash)),
            "│●"
        );
    }
}

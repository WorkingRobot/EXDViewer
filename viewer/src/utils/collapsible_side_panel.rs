use egui::{
    Context, Id, InnerResponse, Response, Sense, Ui, collapsing_header::paint_default_icon,
    panel::Side, pos2, remap, vec2,
};

pub struct CollapsibleSidePanel {
    id: Id,
    side: Side,
    collapsed_width: Option<f32>,
}

impl CollapsibleSidePanel {
    pub fn new(id: impl Into<Id>, side: Side) -> Self {
        Self {
            id: id.into(),
            side,
            collapsed_width: None,
        }
    }

    pub fn collapsed_width(mut self, width: f32) -> Self {
        self.collapsed_width = Some(width);
        self
    }

    pub fn show<R>(
        self,
        ctx: &Context,
        add_contents: impl FnOnce(&mut Ui, bool) -> R,
    ) -> Option<InnerResponse<R>> {
        let is_expanded = !Self::is_collapsed(ctx, self.id);
        let openness = Self::openness(ctx, self.id);

        let collapsed_panel = egui::SidePanel::new(self.side, self.id.with("collapsed"))
            .resizable(false)
            .exact_width(self.collapsed_width.unwrap_or_default());

        if openness != 0.0 || self.collapsed_width.is_some() {
            egui::SidePanel::show_animated_between(
                ctx,
                is_expanded,
                collapsed_panel,
                egui::SidePanel::new(self.side, self.id),
                |ui, openness| add_contents(ui, openness != 0.0),
            )
        } else {
            None
        }
    }

    pub fn is_collapsed(ctx: &Context, id: impl Into<Id>) -> bool {
        ctx.data(|d| {
            d.get_temp(id.into().with("is_collapsed"))
                .unwrap_or_default()
        })
    }

    fn openness(ctx: &Context, id: impl Into<Id>) -> f32 {
        let id = id.into();
        ctx.animate_bool_responsive(id.with("arrow_animation"), !Self::is_collapsed(ctx, id))
    }

    pub fn draw_arrow(ui: &mut egui::Ui, panel_id: impl Into<egui::Id>) -> Response {
        let panel_id = panel_id.into();
        let is_collapsed: bool = Self::is_collapsed(ui.ctx(), panel_id);

        let mut response: egui::Response;

        let prev_item_spacing = ui.spacing_mut().item_spacing;
        ui.spacing_mut().item_spacing.x = 0.0;

        let size = vec2(ui.spacing().indent, ui.spacing().icon_width);
        let (space_id, rect) = ui.allocate_space(size);
        response = ui.interact(rect, space_id, Sense::click());
        if response.clicked() {
            response.mark_changed();
            ui.ctx()
                .data_mut(|d| d.insert_temp(panel_id.with("is_collapsed"), !is_collapsed));
        }

        let (mut icon_rect, _) = ui.spacing().icon_rectangles(response.rect);
        icon_rect.set_center(pos2(
            response.rect.left() + ui.spacing().indent / 2.0,
            response.rect.center().y,
        ));
        let openness = Self::openness(ui.ctx(), panel_id);
        let small_icon_response = response.clone().with_new_rect(icon_rect);
        paint_default_icon(
            ui,
            remap(openness, 0.0..=1.0, 0.0..=2.0),
            &small_icon_response,
        );

        ui.spacing_mut().item_spacing = prev_item_spacing;
        response
    }
}

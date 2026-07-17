use anyhow::Result;
use egui::{Button, Hyperlink, Layout, Margin, RichText, ScrollArea, TextEdit};
use itertools::Itertools;

use crate::{
    about::centered_inline, github::{
        GithubAuth, GithubClient, PrDraft, PrResult, RelayResult, build_auth_start, exchange_code,
        fetch_client_id, relay_and_close, take_relayed_result,
    }, settings::{BACKEND_CONFIG, BackendConfig, GithubSchemaLocation, SchemaLocation}, utils::{PromiseKind, TrackedPromise},
};

pub type PrOutcome = std::result::Result<PrResult, String>;

struct Draft {
    title: String,
    body: String,
    use_token: bool,
}

pub enum PrAction {
    Submit { title: String, body: String },
}

pub fn github_source(ctx: &egui::Context) -> Option<GithubSchemaLocation> {
    match BACKEND_CONFIG.get(ctx) {
        Some(BackendConfig {
            schema: SchemaLocation::Github(location),
            ..
        }) => Some(location),
        _ => None,
    }
}

pub fn draw_auth_callback(ui: &mut egui::Ui) {
    relay_and_close();
    ui.vertical_centered(|ui| {
        ui.add_space(48.0);
        ui.spinner();
        ui.add_space(8.0);
        ui.label("Completing sign-in… you can close this tab.");
    });
}

#[derive(Default)]
pub struct PrWindow {
    github_token: String,
    github_auth: Option<GithubAuth>,
    oauth_client_id: Option<String>,
    client_id_promise: Option<TrackedPromise<Result<String>>>,
    /// (verifier, state)
    oauth_pending: Option<(String, String)>,
    oauth_exchange: Option<TrackedPromise<Result<GithubAuth>>>,
    oauth_error: Option<String>,
    draft: Option<Draft>,
    pr_promise: Option<TrackedPromise<Result<PrResult>>>,
    pr_outcome: Option<PrOutcome>,
}

impl PrWindow {
    pub fn poll(&mut self, ctx: &egui::Context) {
        if self
            .client_id_promise
            .as_ref()
            .is_some_and(|p| p.try_get().is_some())
        {
            match self.client_id_promise.take().unwrap().block_and_take() {
                Ok(id) => self.oauth_client_id = Some(id),
                Err(e) => {
                    log::error!("Failed to fetch OAuth client id: {e}");
                    self.oauth_error = Some(e.to_string());
                }
            }
        }

        if let Some((verifier, state)) = self.oauth_pending.clone() {
            ctx.request_repaint_after(std::time::Duration::from_millis(200));
            match take_relayed_result() {
                Some(RelayResult::Code {
                    code,
                    state: got_state,
                }) => {
                    self.oauth_pending = None;
                    if got_state != state {
                        self.oauth_error = Some("Sign-in failed: state mismatch".to_string());
                    } else {
                        self.oauth_exchange = Some(TrackedPromise::spawn_local(async move {
                            exchange_code(code, verifier).await
                        }));
                    }
                }
                Some(RelayResult::Error(e)) => {
                    self.oauth_pending = None;
                    self.oauth_error = Some(e);
                }
                None => {}
            }
        }

        if self
            .oauth_exchange
            .as_ref()
            .is_some_and(|p| p.try_get().is_some())
        {
            match self.oauth_exchange.take().unwrap().block_and_take() {
                Ok(auth) => {
                    log::info!("Signed in to GitHub as {}", auth.login);
                    self.oauth_error = None;
                    self.github_auth = Some(auth);
                }
                Err(e) => {
                    log::error!("GitHub sign-in failed: {e}");
                    self.oauth_error = Some(e.to_string());
                }
            }
        }
    }

    fn ensure_client_id(&mut self) {
        if self.oauth_client_id.is_none() && self.client_id_promise.is_none() {
            self.client_id_promise = Some(TrackedPromise::spawn_local(async move {
                fetch_client_id().await
            }));
        }
    }

    pub fn open(&mut self, modified_names: &[String]) {
        let title = match modified_names {
            [one] => format!("Update {one} schema"),
            many => format!("Update {} schemas", many.len()),
        };
        let body = format!(
            "Updated schemas:\n{}",
            modified_names.iter().map(|n| format!("- {n}")).join("\n")
        );
        self.pr_outcome = None;
        // Prefetch client id
        if self.github_auth.is_none() {
            self.ensure_client_id();
        }
        self.draft = Some(Draft {
            title,
            body,
            use_token: false,
        });
    }

    fn begin_login(&mut self, ctx: &egui::Context) {
        self.oauth_error = None;
        let Some(client_id) = self.oauth_client_id.clone() else {
            self.ensure_client_id();
            self.oauth_error = Some("Preparing sign-in… try again in a moment".to_string());
            return;
        };
        match build_auth_start(&client_id) {
            Ok(start) => {
                self.oauth_pending = Some((start.verifier, start.state));
                ctx.open_url(egui::OpenUrl::new_tab(start.url));
            }
            Err(e) => {
                log::error!("Failed to start GitHub sign-in: {e}");
                self.oauth_error = Some(e.to_string());
            }
        }
    }

    pub fn submit(
        &mut self,
        location: &GithubSchemaLocation,
        title: String,
        body: String,
        files: Vec<(String, String)>,
    ) {
        if files.is_empty() {
            return;
        }
        let draft = PrDraft {
            base_owner: location.owner.clone(),
            base_repo: location.repo.clone(),
            base_branch: location.base_branch(),
            title,
            body,
            files,
        };
        let token = self
            .github_auth
            .as_ref()
            .map(|a| a.token.clone())
            .unwrap_or_else(|| self.github_token.trim().to_string());
        let client = GithubClient::new(token);
        self.pr_outcome = None;
        self.pr_promise = Some(TrackedPromise::spawn_local(async move {
            client.submit_pr(&draft).await
        }));
    }

    pub fn draw(
        &mut self,
        ctx: &egui::Context,
        location: Option<&GithubSchemaLocation>,
        // (name, invalid_reason)
        modified: &[(String, Option<String>)],
    ) -> Option<PrAction> {
        if self
            .pr_promise
            .as_ref()
            .is_some_and(|p| p.try_get().is_some())
        {
            let result = self.pr_promise.take().unwrap().block_and_take();
            self.pr_outcome = Some(result.map_err(|e| e.to_string()));
        }

        let Some(location) = location else {
            self.draft = None;
            return None;
        };
        let mut window = self.draft.take()?;

        let invalid_count = modified.iter().filter(|(_, r)| r.is_some()).count();
        let submitting = self.pr_promise.is_some();
        let signed_in_as = self.github_auth.as_ref().map(|a| a.login.clone());
        let signing_in = self.oauth_pending.is_some() || self.oauth_exchange.is_some();
        let client_id_ready = self.oauth_client_id.is_some();
        let oauth_error = self.oauth_error.clone();
        let mut open = true;
        let mut submit = false;
        let mut begin_login = false;
        let mut sign_out = false;
        egui::Window::new("Create pull request")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(460.0)
            .show(ctx, |ui| {
                ui.spacing_mut().item_spacing = egui::vec2(8.0, 4.0);
                
                let dest = format!(
                    "{}/{} @ {}",
                    location.owner,
                    location.repo,
                    location.base_branch()
                );
                centered_inline(ui, &format!("Merging into {dest}"), |ui| {
                    ui.label("Merging into ");
                    ui.add(
                        Hyperlink::from_label_and_url(dest, format!(
                            "https://github.com/{}/{}/tree/{}",
                            location.owner,
                            location.repo,
                            location.base_branch()
                        ))
                            .open_in_new_tab(true),
                    );
                });

                if modified.is_empty() {
                    ui.add_space(8.0);
                    ui.label("No modified schemas to submit.");
                    return;
                }

                ui.add_space(10.0);
                ui.label(RichText::new("Title").strong());
                ui.add(TextEdit::singleline(&mut window.title).desired_width(f32::INFINITY));
                ui.add_space(6.0);
                ui.label(RichText::new("Description").strong());
                ui.add(
                    TextEdit::multiline(&mut window.body)
                        .desired_width(f32::INFINITY)
                        .desired_rows(5),
                );

                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Changed schemas").strong());
                    ui.label(RichText::new(format!("({})", modified.len())).weak());
                });
                egui::Frame::group(ui.style())
                    .inner_margin(Margin::symmetric(10, 8))
                    .show(ui, |ui| {
                        ui.set_min_width(ui.available_width());
                        let list = |ui: &mut egui::Ui| {
                            for (name, reason) in modified {
                                match reason {
                                    None => {
                                        ui.label(name);
                                    }
                                    Some(reason) => {
                                        ui.colored_label(ui.visuals().warn_fg_color, name);
                                        ui.label(RichText::new(reason).small().weak());
                                    }
                                }
                            }
                        };
                        if modified.len() > 8 {
                            ScrollArea::vertical()
                                .max_height(150.0)
                                .auto_shrink([false, true])
                                .show(ui, list);
                        } else {
                            list(ui);
                        }
                    });

                ui.add_space(10.0);
                ui.label(RichText::new("Account").strong());
                if let Some(login) = &signed_in_as {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 3.0;
                        ui.label("Signed in as");
                        ui.label(RichText::new(login).strong());
                        ui.with_layout(Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button("Sign out").clicked() {
                                sign_out = true;
                            }
                        });
                    });
                } else if signing_in {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("Waiting for GitHub. Finish in the new tab.");
                    });
                    if let Some(err) = &oauth_error {
                        ui.colored_label(ui.visuals().error_fg_color, err);
                    }
                } else {
                    ui.columns_const(|[col_1, col_2]| {
                        col_1.vertical_centered_justified(|ui| {
                            if ui
                                .add_enabled(
                                    client_id_ready,
                                    Button::new(format!(
                                        "{}  Sign in with GitHub",
                                        egui::special_emojis::GITHUB
                                    )),
                                )
                                .clicked()
                            {
                                begin_login = true;
                                window.use_token = false;
                            }
                        });
                        col_2.vertical_centered_justified(|ui| {
                            if ui.button("Use a token instead").clicked() {
                                window.use_token = !window.use_token;
                            }
                        });
                    });

                    if window.use_token {
                        ui.add(
                            TextEdit::singleline(&mut self.github_token)
                                .password(true)
                                .hint_text("ghp_… personal access token")
                                .desired_width(f32::INFINITY),
                        );
                        ui.horizontal_wrapped(|ui| {
                            ui.spacing_mut().item_spacing.x = 3.0;
                            let small = egui::TextStyle::Small.resolve(ui.style()).size;

                            ui.label(RichText::new("Needs").small().weak());
                            ui.label(RichText::new("public_repo").monospace().size(small));
                            ui.label(RichText::new("scope.").small().weak());
                            ui.add(
                                egui::Hyperlink::from_label_and_url(
                                    RichText::new("Create one").small(),
                                    crate::CREATE_PAT_URL,
                                ).open_in_new_tab(true),
                            );
                        });
                    } else if !client_id_ready {
                        ui.label(RichText::new("Preparing sign-in…").small().weak());
                    } else if let Some(err) = &oauth_error {
                        ui.colored_label(ui.visuals().error_fg_color, err);
                    }
                }

                let authenticated =
                    signed_in_as.is_some() || !self.github_token.trim().is_empty();
                let can_submit = !submitting
                    && invalid_count == 0
                    && authenticated
                    && !window.title.trim().is_empty();

                ui.add_space(10.0);
                ui.separator();
                ui.add_space(2.0);
                ui.horizontal(|ui| {
                    ui.with_layout(Layout::right_to_left(egui::Align::Center), |ui| {
                        // Once a PR is open, the primary action becomes "view" so the same
                        // changes can't be submitted twice. Reopening the dialog clears the
                        // outcome and allows a fresh submission.
                        if let Some(Ok(pr)) = &self.pr_outcome {
                            if ui.button("View pull request").clicked() {
                                ui.ctx().open_url(egui::OpenUrl::new_tab(pr.html_url.clone()));
                            }
                        } else if ui
                            .add_enabled(can_submit, Button::new("Create pull request"))
                            .clicked()
                        {
                            submit = true;
                        }
                        if submitting {
                            ui.spinner();
                            ui.label(RichText::new("Submitting…").weak());
                        }
                        ui.with_layout(Layout::left_to_right(egui::Align::Center), |ui| {
                            match &self.pr_outcome {
                                Some(Ok(pr)) => {
                                    ui.label(
                                        RichText::new(format!(
                                            "✅ Pull request #{} opened",
                                            pr.number
                                        ))
                                        .strong(),
                                    );
                                }
                                Some(Err(err)) => {
                                    ui.colored_label(
                                        ui.visuals().error_fg_color,
                                        format!("❌ {err}"),
                                    );
                                }
                                None if invalid_count > 0 => {
                                    ui.colored_label(
                                        ui.visuals().warn_fg_color,
                                        format!(
                                            "Fix {invalid_count} invalid schema{} before submitting.",
                                            if invalid_count == 1 { "" } else { "s" }
                                        ),
                                    );
                                }
                                None => {}
                            }
                        });
                    });
                });
            });

        if begin_login {
            self.begin_login(ctx);
        }
        if sign_out {
            self.github_auth = None;
        }
        let action = submit.then(|| PrAction::Submit {
            title: window.title.clone(),
            body: window.body.clone(),
        });
        if open {
            self.draft = Some(window);
        }
        action
    }
}

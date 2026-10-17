//! Screen views. Plain language, big targets, no jargon unless the user asks
//! for details.

use crate::app::{App, Focusable, Message, Screen};
use crate::style::{self, BODY, HEADING, MONO, SMALL, TITLE};
use crate::worker::Action;
use iced::widget::{
    Space, button, checkbox, column, container, image, progress_bar, row, rule, scrollable, svg, text, text_input,
};
use iced::{Alignment, Element, Fill, Font, Length};
use installer_core::InstallStatus;

pub fn view(app: &App) -> Element<'_, Message> {
    let body: Element<'_, Message> = match app.screen {
        Screen::Loading => loading(),
        Screen::PickManifest => pick_manifest(app),
        Screen::Welcome => welcome(app),
        Screen::Manage => manage(app),
        Screen::Progress => progress(app),
        Screen::Finished => finished(app),
        Screen::Error => error(app),
    };
    let content = column![header(app), body].spacing(20).max_width(760).width(Fill);
    container(scrollable(container(content).padding([24, 32]).width(Fill)))
        .width(Fill)
        .height(Fill)
        .into()
}

#[derive(Clone, Copy)]
enum Kind {
    Primary,
    Secondary,
    Danger,
    Link,
}

fn btn<'a>(app: &App, label: impl Into<String>, target: Focusable, kind: Kind) -> Element<'a, Message> {
    let focused = app.focused() == Some(target);
    let label: String = label.into();
    let b = button(text(label).size(BODY))
        .padding([10, 22])
        .on_press(Message::Activate(target));
    match kind {
        Kind::Primary => b.style(style::primary(focused)),
        Kind::Secondary => b.style(style::secondary(focused)),
        Kind::Danger => b.style(style::danger(focused)),
        Kind::Link => b.style(style::link(focused)).padding([6, 10]),
    }
    .into()
}

fn big_button<'a>(app: &App, label: impl Into<String>, target: Focusable) -> Element<'a, Message> {
    let focused = app.focused() == Some(target);
    let label: String = label.into();
    button(text(label).size(21).width(Fill).center())
        .padding([16, 40])
        .width(Length::Fixed(300.0))
        .on_press(Message::Activate(target))
        .style(style::primary(focused))
        .into()
}

fn header(app: &App) -> Element<'_, Message> {
    let icon: Element<'_, Message> = if let Some(h) = &app.icon {
        image(h.clone()).width(72).height(72).into()
    } else if let Some(h) = &app.icon_svg {
        svg(h.clone()).width(72).height(72).into()
    } else {
        image(image::Handle::from_bytes(
            include_bytes!("../../../assets/icon-128.png").as_slice(),
        ))
        .width(72)
        .height(72)
        .into()
    };
    let (name, sub) = match &app.manifest {
        Some(m) => {
            let a = &m.manifest.app;
            let mut parts = vec![format!("Version {}", a.version)];
            if let Some(p) = &a.publisher {
                parts.push(p.clone());
            }
            (a.display_name().to_string(), parts.join("  •  "))
        }
        None => ("Omnistall".to_string(), "Application installer".to_string()),
    };
    let mut titles = column![
        text(name).size(TITLE),
        text(sub).size(SMALL).style(|t| text::Style {
            color: Some(style::muted(t))
        })
    ]
    .spacing(2);
    if let Some(home) = app.manifest.as_ref().and_then(|m| m.manifest.app.homepage.clone()) {
        titles = titles.push(
            button(text(home).size(SMALL))
                .style(button::text)
                .padding(0)
                .on_press(Message::OpenHomepage),
        );
    }
    row![icon, titles].spacing(18).align_y(Alignment::Center).into()
}

fn loading<'a>() -> Element<'a, Message> {
    column![
        text("Getting ready…").size(HEADING),
        progress_bar(0.0..=1.0, 0.15).girth(10)
    ]
    .spacing(16)
    .into()
}

fn pick_manifest(app: &App) -> Element<'_, Message> {
    let mut col = column![
        text("Which app should be installed?").size(HEADING),
        text("This installer was not bundled with an app description. Enter the location of a manifest file (.toml): a path on this computer or a web address.").size(BODY),
        text_input("https://example.com/myapp.toml  or  C:\\path\\myapp.toml", &app.manifest_input)
            .id("manifest-input")
            .size(BODY)
            .padding(12)
            .on_input(Message::ManifestInput)
            .on_submit(Message::LoadManifest),
    ]
    .spacing(14);
    if let Some(e) = &app.load_error {
        col = col.push(
            text(format!("Could not load that manifest: {e}"))
                .size(SMALL)
                .style(|t| text::Style {
                    color: Some(style::error(t)),
                }),
        );
    }
    col = col.push(
        row![
            btn(app, "Continue", Focusable::Continue, Kind::Primary),
            Space::new().width(Fill),
            btn(app, "Close", Focusable::Close, Kind::Secondary)
        ]
        .align_y(Alignment::Center),
    );
    col.into()
}

fn welcome(app: &App) -> Element<'_, Message> {
    let Some(m) = &app.manifest else { return loading() };
    let a = &m.manifest.app;
    let mut col = column![].spacing(18);
    if let Some(d) = &a.description {
        col = col.push(text(d.clone()).size(BODY));
    }
    col = col.push(
        container(big_button(app, "Install", Focusable::Install))
            .center_x(Fill)
            .padding([10, 0]),
    );

    let dir_note = if app.is_cli() {
        format!(
            "Installs for your user only. The `{}` command becomes available in new terminal windows.",
            app.command_name()
        )
    } else {
        "Installs for your user only. No administrator password needed.".to_string()
    };
    col = col.push(text(dir_note).size(SMALL).style(|t| text::Style {
        color: Some(style::muted(t)),
    }));

    let toggle_label = if app.options_open { "▾ Options" } else { "▸ Options" };
    col = col.push(btn(app, toggle_label, Focusable::Options, Kind::Link));
    if app.options_open {
        col = col.push(options_card(app));
    }
    col = col.push(Space::new().height(8));
    col = col.push(row![
        Space::new().width(Fill),
        btn(app, "Close", Focusable::Close, Kind::Secondary)
    ]);
    col.into()
}

fn options_card(app: &App) -> Element<'_, Message> {
    let menu_label = if cfg!(target_os = "windows") {
        "Add to the Start Menu"
    } else if cfg!(target_os = "macos") {
        "Add to Applications (Launchpad)"
    } else {
        "Add to the application menu"
    };
    let mut col = column![
        text("Install location").size(BODY),
        text_input(&app.default_dir, &app.install_dir_text)
            .size(BODY)
            .padding(10)
            .on_input(Message::InstallDirChanged),
        text(format!("Default: {}", app.default_dir))
            .size(SMALL)
            .style(|t| text::Style {
                color: Some(style::muted(t))
            }),
        rule::horizontal(1),
        checkbox(app.options.desktop_shortcut)
            .label("Create a Desktop shortcut")
            .text_size(BODY)
            .on_toggle(Message::DesktopShortcut),
        checkbox(app.options.menu_shortcut)
            .label(menu_label)
            .text_size(BODY)
            .on_toggle(Message::MenuShortcut),
    ]
    .spacing(12);
    let path_default = app.is_cli();
    col = col.push(
        checkbox(app.options.add_to_path.unwrap_or(path_default))
            .label("Make the command available in terminals (add to PATH)")
            .text_size(BODY)
            .on_toggle(Message::AddToPath),
    );
    if app.manifest.as_ref().is_some_and(|m| m.manifest.build.is_some()) {
        col = col.push(
            checkbox(app.options.from_source)
                .label("Build from source code instead of downloading (slower, needs build tools)")
                .text_size(BODY)
                .on_toggle(Message::FromSource),
        );
    }
    container(col).padding(18).style(style::card).width(Fill).into()
}

fn manage(app: &App) -> Element<'_, Message> {
    let Some(InstallStatus::Installed {
        app: inst,
        update_available,
    }) = &app.status
    else {
        return welcome(app);
    };
    let name = app.app_name();
    let new_version = app
        .manifest
        .as_ref()
        .map(|m| m.manifest.app.version.clone())
        .unwrap_or_default();
    let mut col = column![text(format!("{name} is already installed")).size(HEADING)].spacing(16);
    let mut info = column![text(format!("Installed version: {}", inst.version)).size(BODY)].spacing(6);
    if let Some(d) = &inst.install_dir {
        info = info.push(
            text(format!("Location: {}", d.display()))
                .size(SMALL)
                .style(|t| text::Style {
                    color: Some(style::muted(t)),
                }),
        );
    }
    if *update_available {
        info = info.push(
            text(format!("Version {new_version} is available."))
                .size(BODY)
                .style(|t| text::Style {
                    color: Some(style::success(t)),
                }),
        );
    }
    col = col.push(container(info).padding(18).style(style::card).width(Fill));

    let mut actions = row![].spacing(12).align_y(Alignment::Center);
    if *update_available {
        actions = actions.push(btn(
            app,
            format!("Update to {new_version}"),
            Focusable::Update,
            Kind::Primary,
        ));
        actions = actions.push(btn(app, "Repair", Focusable::Repair, Kind::Secondary));
    } else {
        actions = actions.push(btn(app, "Repair", Focusable::Repair, Kind::Primary));
    }
    actions = actions.push(btn(app, "Uninstall", Focusable::Uninstall, Kind::Danger));
    actions = actions.push(Space::new().width(Fill));
    actions = actions.push(btn(app, "Close", Focusable::Close, Kind::Secondary));
    col = col.push(actions);
    col = col.push(
        text("Repair reinstalls the same version and fixes shortcuts. Uninstall removes the app, its shortcuts and PATH entries.")
            .size(SMALL)
            .style(|t| text::Style { color: Some(style::muted(t)) }),
    );
    col.into()
}

fn progress(app: &App) -> Element<'_, Message> {
    let p = &app.progress;
    let stage = p.stage.map(|s| s.label()).unwrap_or("Starting…");
    let mut col = column![
        text(format!("{} {}…", app.action.gerund(), app.app_name())).size(HEADING),
        progress_bar(0.0..=1.0, p.fraction).girth(14),
    ]
    .spacing(14);

    let detail = match (p.bytes_total, p.bytes_done) {
        (Some(t), d) if t > 0 && p.stage == Some(installer_core::Stage::Downloading) => {
            format!("{stage}  {} of {}", human_bytes(d), human_bytes(t))
        }
        _ => stage.to_string(),
    };
    col = col.push(text(detail).size(BODY));

    if let Some((q, _)) = &app.question {
        let card = column![
            text(q.title.clone()).size(HEADING),
            text(q.body.clone()).size(BODY),
            row![
                btn(app, q.accept_label.clone(), Focusable::Yes, Kind::Primary),
                btn(app, q.decline_label.clone(), Focusable::No, Kind::Secondary),
            ]
            .spacing(12),
        ]
        .spacing(12);
        col = col.push(container(card).padding(18).style(style::card).width(Fill));
    } else {
        let details_label = if app.show_details {
            "Hide details"
        } else {
            "Show details"
        };
        col = col.push(
            row![
                btn(app, "Cancel", Focusable::Cancel, Kind::Secondary),
                Space::new().width(Fill),
                btn(app, details_label, Focusable::Details, Kind::Link),
            ]
            .align_y(Alignment::Center),
        );
    }
    if app.show_details {
        col = col.push(log_view(&p.logs, 200.0));
    }
    col.into()
}

fn log_view<'a>(logs: &[String], height: f32) -> Element<'a, Message> {
    let body = logs.join("\n");
    container(
        scrollable(text(body).size(MONO).font(Font::MONOSPACE))
            .anchor_bottom()
            .height(Length::Fixed(height))
            .width(Fill),
    )
    .padding(10)
    .style(style::log_box)
    .width(Fill)
    .into()
}

fn finished(app: &App) -> Element<'_, Message> {
    let name = app.app_name();
    let headline = match app.action {
        Action::Uninstall => format!("{name} was removed"),
        Action::Update => format!("{name} is up to date"),
        Action::Repair => format!("{name} has been repaired"),
        Action::Install => format!("{name} is ready"),
    };
    let mut col = column![
        row![
            text("✓").size(TITLE).style(|t| text::Style {
                color: Some(style::success(t))
            }),
            text(headline).size(HEADING)
        ]
        .spacing(12)
        .align_y(Alignment::Center),
    ]
    .spacing(16);

    if app.action != Action::Uninstall {
        if app.is_cli() {
            let cmd = app.command_name();
            col = col.push(text(format!("Open a new terminal window and type `{cmd}` to use it. Already-open terminals need to be restarted first.")).size(BODY));
        } else if let Some(inst) = &app.result {
            if let Some(d) = &inst.install_dir {
                col = col.push(
                    text(format!("Installed in {}", d.display()))
                        .size(SMALL)
                        .style(|t| text::Style {
                            color: Some(style::muted(t)),
                        }),
                );
            } else if let Some(b) = &inst.app_bundle {
                col = col.push(
                    text(format!("Installed in {}", b.display()))
                        .size(SMALL)
                        .style(|t| text::Style {
                            color: Some(style::muted(t)),
                        }),
                );
            }
        }
    }
    if !app.progress.warnings.is_empty() {
        let mut w = column![text("Notes:").size(SMALL)].spacing(4);
        for line in &app.progress.warnings {
            w = w.push(text(format!("• {line}")).size(SMALL).style(|t| text::Style {
                color: Some(style::muted(t)),
            }));
        }
        col = col.push(container(w).padding(14).style(style::card).width(Fill));
    }
    if let Some(e) = &app.launch_error {
        col = col.push(
            text(format!("Could not launch: {e}"))
                .size(SMALL)
                .style(|t| text::Style {
                    color: Some(style::error(t)),
                }),
        );
    }
    let mut actions = row![].spacing(12).align_y(Alignment::Center);
    if app.result.is_some() && !app.is_cli() {
        actions = actions.push(btn(app, format!("Launch {name}"), Focusable::Launch, Kind::Primary));
    }
    actions = actions.push(Space::new().width(Fill));
    actions = actions.push(btn(app, "Close", Focusable::Close, Kind::Secondary));
    col = col.push(actions);
    col.into()
}

fn error(app: &App) -> Element<'_, Message> {
    let friendly = app
        .failure
        .as_ref()
        .map(|f| f.friendly.clone())
        .unwrap_or_else(|| "Something went wrong.".into());
    let technical = app.failure.as_ref().map(|f| f.technical.clone()).unwrap_or_default();
    let mut col = column![
        text("Something went wrong").size(HEADING).style(|t| text::Style { color: Some(style::error(t)) }),
        text(friendly).size(BODY),
        text("You can try again. If the problem continues, use “Copy details” and send them to whoever provided this installer.")
            .size(SMALL)
            .style(|t| text::Style { color: Some(style::muted(t)) }),
    ]
    .spacing(14);
    let copy_label = if app.copied { "Copied!" } else { "Copy details" };
    let details_label = if app.show_details {
        "Hide details"
    } else {
        "Show details"
    };
    col = col.push(
        row![
            btn(app, "Try again", Focusable::Retry, Kind::Primary),
            btn(app, copy_label, Focusable::CopyDetails, Kind::Secondary),
            btn(app, details_label, Focusable::Details, Kind::Link),
            Space::new().width(Fill),
            btn(app, "Back", Focusable::Back, Kind::Secondary),
            btn(app, "Close", Focusable::Close, Kind::Secondary),
        ]
        .spacing(10)
        .align_y(Alignment::Center),
    );
    if app.show_details {
        let mut lines = vec![format!("Error: {technical}")];
        lines.extend(app.progress.logs.iter().rev().take(60).rev().cloned());
        col = col.push(log_view(&lines, 180.0));
    }
    col.into()
}

fn human_bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = n as f64;
    let mut i = 0;
    while v >= 1000.0 && i < UNITS.len() - 1 {
        v /= 1000.0;
        i += 1;
    }
    if i == 0 {
        format!("{n} B")
    } else {
        format!("{v:.1} {}", UNITS[i])
    }
}

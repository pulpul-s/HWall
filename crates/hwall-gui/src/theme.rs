use hwall_app::ThemePreference;

pub(super) fn apply(preference: ThemePreference) {
    let Some(settings) = gtk::Settings::default() else {
        return;
    };

    match preference {
        ThemePreference::System => {
            settings.reset_property("gtk-theme-name");
            settings.reset_property("gtk-application-prefer-dark-theme");
        }
        ThemePreference::Light => {
            settings.set_gtk_theme_name(Some("Adwaita"));
            settings.set_gtk_application_prefer_dark_theme(false);
        }
        ThemePreference::Dark => {
            settings.set_gtk_theme_name(Some("Adwaita"));
            settings.set_gtk_application_prefer_dark_theme(true);
        }
    }
}

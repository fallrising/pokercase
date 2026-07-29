use std::sync::OnceLock;

use minijinja::{Environment, Value};

static ENV: OnceLock<Environment<'static>> = OnceLock::new();

pub fn env() -> &'static Environment<'static> {
    ENV.get_or_init(|| {
        let mut env = Environment::new();
        env.add_template("dashboard", include_str!("../templates/dashboard.html"))
            .unwrap();
        env.add_template(
            "connections",
            include_str!("../templates/connections.html"),
        )
        .unwrap();
        env.add_template("routes", include_str!("../templates/routes.html"))
            .unwrap();
        env.add_template("keys", include_str!("../templates/keys.html"))
            .unwrap();
        env.add_template("usage", include_str!("../templates/usage.html"))
            .unwrap();
        env.add_template("login", include_str!("../templates/login.html"))
            .unwrap();
        env
    })
}

pub fn render(name: &str, ctx: Value) -> Result<String, minijinja::Error> {
    let tmpl = env().get_template(name)?;
    tmpl.render(ctx)
}

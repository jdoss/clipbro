use std::path::PathBuf;

const UNIT_NAME: &str = "clipbro.service";
const SYSTEMD_BUS: &str = "org.freedesktop.systemd1";
const SYSTEMD_PATH: &str = "/org/freedesktop/systemd1";
const MANAGER_IFACE: &str =
    "org.freedesktop.systemd1.Manager";

fn unit_dir() -> PathBuf {
    let home = std::env::var("HOME")
        .unwrap_or_else(|_| "/tmp".into());
    PathBuf::from(home)
        .join(".config/systemd/user")
}

fn unit_path() -> PathBuf {
    unit_dir().join(UNIT_NAME)
}

fn unit_contents() -> String {
    let exe = std::env::current_exe()
        .unwrap_or_else(|_| PathBuf::from("clipbro"));
    format!(
        "\
[Unit]
Description=Clipbro clipboard manager
After=graphical-session.target

[Service]
Type=simple
ExecStart={exe}
Restart=on-failure
RestartSec=3

[Install]
WantedBy=graphical-session.target
",
        exe = exe.display()
    )
}

pub fn install() {
    let path = unit_path();
    if path.exists() {
        eprintln!(
            "Unit file already exists: {}",
            path.display()
        );
        eprintln!("Rewriting with current binary path.");
    }

    let dir = unit_dir();
    if let Err(e) = std::fs::create_dir_all(&dir) {
        eprintln!(
            "Failed to create {}: {e}",
            dir.display()
        );
        std::process::exit(1);
    }

    if let Err(e) = std::fs::write(&path, unit_contents()) {
        eprintln!(
            "Failed to write {}: {e}",
            path.display()
        );
        std::process::exit(1);
    }
    eprintln!("Wrote unit file: {}", path.display());

    if let Err(e) = daemon_reload() {
        eprintln!("Failed to reload systemd: {e}");
        std::process::exit(1);
    }
    eprintln!("Reloaded systemd user manager.");

    if let Err(e) = enable_unit() {
        eprintln!("Failed to enable unit: {e}");
        std::process::exit(1);
    }
    eprintln!("Enabled {UNIT_NAME}.");
}

pub fn start() {
    if let Err(e) = unit_action("StartUnit") {
        eprintln!("Failed to start {UNIT_NAME}: {e}");
        std::process::exit(1);
    }
    eprintln!("Started {UNIT_NAME}.");
}

pub fn stop() {
    if let Err(e) = unit_action("StopUnit") {
        eprintln!("Failed to stop {UNIT_NAME}: {e}");
        std::process::exit(1);
    }
    eprintln!("Stopped {UNIT_NAME}.");
}

pub fn restart() {
    if let Err(e) = unit_action("RestartUnit") {
        eprintln!("Failed to restart {UNIT_NAME}: {e}");
        std::process::exit(1);
    }
    eprintln!("Restarted {UNIT_NAME}.");
}

pub fn status() {
    let installed = unit_path().exists();
    println!("Unit file: {}", if installed {
        unit_path().display().to_string()
    } else {
        "not installed".to_string()
    });

    match get_unit_properties() {
        Ok((load, active, sub, pid)) => {
            println!("Loaded:    {load}");
            println!("Active:    {active} ({sub})");
            if pid > 0 {
                println!("PID:       {pid}");
            }
        }
        Err(e) => {
            let msg = e.to_string();
            if msg.contains("NoSuchUnit")
                || msg.contains("not loaded")
            {
                println!("Loaded:    not-found");
                println!("Active:    inactive");
            } else {
                eprintln!("Failed to query status: {e}");
                std::process::exit(1);
            }
        }
    }
}

fn get_unit_properties()
    -> Result<(String, String, String, u32), zbus::Error>
{
    let conn = session_conn()?;

    let reply = conn.call_method(
        Some(SYSTEMD_BUS),
        SYSTEMD_PATH,
        Some(MANAGER_IFACE),
        "GetUnit",
        &(UNIT_NAME,),
    )?;
    let unit_path: zbus::zvariant::OwnedObjectPath =
        reply.body().deserialize()?;

    let prop_iface =
        "org.freedesktop.DBus.Properties";
    let unit_iface =
        "org.freedesktop.systemd1.Unit";
    let svc_iface =
        "org.freedesktop.systemd1.Service";

    let load: String = get_property(
        &conn, &unit_path, prop_iface, unit_iface,
        "LoadState",
    )?;
    let active: String = get_property(
        &conn, &unit_path, prop_iface, unit_iface,
        "ActiveState",
    )?;
    let sub: String = get_property(
        &conn, &unit_path, prop_iface, unit_iface,
        "SubState",
    )?;
    let pid: u32 = get_property(
        &conn, &unit_path, prop_iface, svc_iface,
        "MainPID",
    )?;

    Ok((load, active, sub, pid))
}

fn get_property<T>(
    conn: &zbus::blocking::Connection,
    path: &zbus::zvariant::OwnedObjectPath,
    prop_iface: &str,
    target_iface: &str,
    prop_name: &str,
) -> Result<T, zbus::Error>
where
    T: TryFrom<zbus::zvariant::OwnedValue>,
    T::Error: std::fmt::Display,
{
    let reply = conn.call_method(
        Some(SYSTEMD_BUS),
        path.as_ref(),
        Some(prop_iface),
        "Get",
        &(target_iface, prop_name),
    )?;
    let val: zbus::zvariant::OwnedValue =
        reply.body().deserialize()?;
    T::try_from(val).map_err(|e| {
        zbus::Error::Failure(format!(
            "Property {prop_name}: {e}"
        ))
    })
}

fn session_conn() -> Result<
    zbus::blocking::Connection,
    zbus::Error,
> {
    zbus::blocking::Connection::session()
}

fn daemon_reload() -> Result<(), zbus::Error> {
    let conn = session_conn()?;
    conn.call_method(
        Some(SYSTEMD_BUS),
        SYSTEMD_PATH,
        Some(MANAGER_IFACE),
        "Reload",
        &(),
    )?;
    Ok(())
}

fn unit_action(method: &str) -> Result<(), zbus::Error> {
    let conn = session_conn()?;
    conn.call_method(
        Some(SYSTEMD_BUS),
        SYSTEMD_PATH,
        Some(MANAGER_IFACE),
        method,
        &(UNIT_NAME, "replace"),
    )?;
    Ok(())
}

fn enable_unit() -> Result<(), zbus::Error> {
    let conn = session_conn()?;
    conn.call_method(
        Some(SYSTEMD_BUS),
        SYSTEMD_PATH,
        Some(MANAGER_IFACE),
        "EnableUnitFiles",
        &(
            vec![UNIT_NAME],
            false, // runtime-only
            true,  // force
        ),
    )?;
    Ok(())
}

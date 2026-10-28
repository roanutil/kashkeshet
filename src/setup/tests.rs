use super::*;
use std::path::Path;

const FILE: &str = "/home/sam/.config/kkt/setup.toml";

fn record(mode: Mode) -> Record {
    Record {
        mode,
        written_by: "0.4.0".into(),
        url: "https://kkt.tail1234.ts.net/mcp".into(),
        key_file: Some("/home/sam/.config/kkt/docker-access-key".into()),
    }
}

fn docker() -> Mode {
    Mode::Docker(DockerSetup {
        folder: "/home/sam/.local/share/kashkeshet/docker".into(),
        tailscale: true,
        hostname: Some("kkt-1".into()),
        tailnet: Some("tail1234.ts.net".into()),
    })
}

fn parse(text: &str) -> Result<Record, String> {
    Record::parse(text, Path::new(FILE))
}

#[test]
fn a_docker_record_is_written_as_documented_and_read_back() {
    let r = record(docker());
    let text = r.render().expect("render");
    assert_eq!(
        text,
        "format = 1\n\
         mode = \"docker\"\n\
         written_by = \"0.4.0\"\n\
         url = \"https://kkt.tail1234.ts.net/mcp\"\n\
         key_file = \"/home/sam/.config/kkt/docker-access-key\"\n\
         \n\
         [docker]\n\
         folder = \"/home/sam/.local/share/kashkeshet/docker\"\n\
         tailscale = true\n\
         hostname = \"kkt-1\"\n\
         tailnet = \"tail1234.ts.net\"\n"
    );
    assert_eq!(parse(&text), Ok(r));
}

#[test]
fn every_mode_reads_back_as_written() {
    let bare_docker = Mode::Docker(DockerSetup {
        folder: "/srv/kkt".into(),
        tailscale: false,
        hostname: None,
        tailnet: None,
    });
    for mode in [
        Mode::Service { tailscale: false },
        Mode::Service { tailscale: true },
        bare_docker,
        Mode::Client,
    ] {
        for key_file in [None, Some("/k".into())] {
            let r = Record {
                key_file,
                ..record(mode.clone())
            };
            assert_eq!(parse(&r.render().expect("render")), Ok(r));
        }
    }
}

#[test]
fn bad_records_are_refused_naming_the_file_and_the_problem() {
    let base = "written_by = \"0.4.0\"\nurl = \"http://127.0.0.1:7420/mcp\"\n";
    let cases = [
        ("format = 2\nmode = \"client\"\n", "has format 2"),
        ("format = 1\nmode = \"cloud\"\n", "unknown mode \"cloud\""),
        (
            "format = 1\nmode = \"docker\"\n",
            "needs a [docker] table and no [service] table",
        ),
        (
            "format = 1\nmode = \"client\"\n[service]\ntailscale = false\n",
            "has no [service] or [docker] table",
        ),
        (
            "format = 1\nmode = \"client\"\nkey_file = \"keys/k\"\n",
            "relative path keys/k",
        ),
        (
            "format = 1\nmode = \"client\"\ncolour = \"blue\"\n",
            "is not valid",
        ),
    ];
    for (start, expected) in cases {
        // Keys before any table, so `base` is not read as part of one.
        let text = match start.split_once('[') {
            Some((top, tables)) => format!("{top}{base}[{tables}"),
            None => format!("{start}{base}"),
        };
        let error = parse(&text).expect_err(&text);
        assert!(error.contains(FILE), "{error}");
        assert!(error.contains(expected), "{error}");
    }
}

#[test]
fn the_record_lives_in_the_config_directory() {
    let env = Env {
        home: Some("/home/sam".into()),
        xdg_config_home: None,
        xdg_data_home: None,
    };
    assert_eq!(record_path(&env), Some(FILE.into()));
    let none = Env {
        home: None,
        xdg_config_home: None,
        xdg_data_home: None,
    };
    assert_eq!(record_path(&none), None);
}

#[test]
fn serve_runs_with_no_record_or_when_told_to_ignore_it() {
    assert_eq!(guard(None, None, false), Ok(()));
    for mode in [Mode::Service { tailscale: false }, docker(), Mode::Client] {
        assert_eq!(guard(Some(&record(mode)), Some(true), true), Ok(()));
    }
}

#[test]
fn with_the_service_set_up_serve_waits_for_it_to_stop() {
    let r = record(Mode::Service { tailscale: false });
    let error = guard(Some(&r), Some(true), false).expect_err("running");
    assert!(error.contains("kkt status"), "{error}");
    assert!(error.contains("--ignore-setup"), "{error}");
    assert_eq!(guard(Some(&r), Some(false), false), Ok(()));
    // Not known: serve tries, and a port in use says so itself.
    assert_eq!(guard(Some(&r), None, false), Ok(()));
}

#[test]
fn docker_and_client_machines_say_where_the_server_is() {
    let error = guard(Some(&record(docker())), Some(false), false).expect_err("docker");
    assert!(
        error.contains("Docker, from /home/sam/.local/share/kashkeshet/docker"),
        "{error}"
    );
    let error = guard(Some(&record(Mode::Client)), None, false).expect_err("client");
    assert!(
        error.contains("client of the kkt server at https://kkt.tail1234.ts.net/mcp"),
        "{error}"
    );
    assert!(error.contains("--ignore-setup"), "{error}");
}

#[test]
fn with_no_one_to_ask_each_question_names_its_flag() {
    let q = Question {
        text: "What is this machine for?",
        flag: "--mode",
    };
    let error = Unattended.choose(q, &["service"]).expect_err("no terminal");
    assert_eq!(
        error,
        "What is this machine for? No terminal to ask on: pass --mode."
    );
    assert!(Unattended.confirm(q).is_err());
    assert!(Unattended.text(q).is_err());
}

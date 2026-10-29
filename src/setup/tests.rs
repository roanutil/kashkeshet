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
fn with_no_one_to_ask_a_question_names_its_flag() {
    let q = Question {
        text: "What is this machine for?",
        flag: "--mode",
        default: None,
    };
    assert_eq!(
        unanswered(q),
        "What is this machine for? No terminal to ask on: pass --mode."
    );
}

mod flow {
    use super::super::*;
    use crate::clients::Client;
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::path::{Path, PathBuf};

    /// An answer a scripted person gives.
    enum Reply {
        Choose(usize),
        Confirm(bool),
        Text(&'static str),
    }

    /// Answers from a script, and keeps the flags of the questions asked
    /// and everything told.
    #[derive(Default)]
    struct Script {
        replies: RefCell<VecDeque<Reply>>,
        asked: RefCell<Vec<String>>,
        defaults: RefCell<Vec<Option<bool>>>,
        told: RefCell<Vec<String>>,
    }

    impl Script {
        fn with(replies: Vec<Reply>) -> Script {
            Script {
                replies: RefCell::new(replies.into()),
                ..Script::default()
            }
        }

        fn next(&self, question: Question) -> Reply {
            self.asked.borrow_mut().push(question.flag.to_string());
            self.defaults.borrow_mut().push(question.default);
            self.replies
                .borrow_mut()
                .pop_front()
                .unwrap_or_else(|| panic!("no scripted reply for: {}", question.text))
        }
    }

    impl Ask for Script {
        fn choose(
            &self,
            question: Question,
            options: &[&str],
            default: usize,
        ) -> Result<usize, String> {
            assert_eq!(Purpose::ALL[default], Purpose::Docker);
            match self.next(question) {
                Reply::Choose(i) if i < options.len() => Ok(i),
                _ => panic!("expected a choice for {}", question.text),
            }
        }
        fn confirm(&self, question: Question) -> Result<bool, String> {
            match self.next(question) {
                Reply::Confirm(yes) => Ok(yes),
                _ => panic!("expected yes or no for {}", question.text),
            }
        }
        fn text(&self, question: Question) -> Result<String, String> {
            match self.next(question) {
                Reply::Text(text) => Ok(text.to_string()),
                _ => panic!("expected text for {}", question.text),
            }
        }
        fn tell(&self, text: &str) {
            self.told.borrow_mut().push(text.to_string());
        }
    }

    /// Steps that record what was done, in order, and fail where told.
    #[derive(Default)]
    struct Fake {
        done: RefCell<Vec<String>>,
        clients: Vec<Client>,
        fail_install: bool,
        fail_connect: Option<Client>,
        fail_doctor: bool,
        /// Addresses where no server answers.
        bad_addresses: Vec<&'static str>,
        /// Keys the server refuses, as [`shown`] shows them.
        bad_keys: Vec<String>,
        /// An address whose server passes the first check and then stops
        /// answering, so every later check of it fails.
        flaky: Option<&'static str>,
        /// The read-only checks made, in order.
        checked: RefCell<Vec<String>>,
    }

    impl Fake {
        fn did(&self, what: String) {
            self.done.borrow_mut().push(what);
        }
        fn done(&self) -> Vec<String> {
            self.done.borrow().clone()
        }
        fn recorded(&self) -> bool {
            self.done().iter().any(|d| d.starts_with("record"))
        }
        fn checked(&self) -> Vec<String> {
            self.checked.borrow().clone()
        }
    }

    /// How the fake shows a key it checks.
    fn shown(key: &ClientKey) -> String {
        match key {
            ClientKey::None => "none".into(),
            ClientKey::File(path) => format!("file {}", path.display()),
            ClientKey::Typed(key) => format!("typed {key}"),
        }
    }

    impl Steps for Fake {
        fn install_service(
            &self,
            tailscale: bool,
            allow_hosts: &[String],
        ) -> Result<Installed, String> {
            self.did(format!(
                "install service tailscale={tailscale}{}",
                hosts(allow_hosts)
            ));
            if self.fail_install {
                return Err("launchctl failed".into());
            }
            Ok(Installed {
                url: "http://127.0.0.1:7420/mcp".into(),
                key_file: Some("/home/sam/.config/kkt/access-key".into()),
            })
        }
        fn install_docker(
            &self,
            tailscale: bool,
            allow_hosts: &[String],
        ) -> Result<(Installed, DockerSetup), String> {
            self.did(format!(
                "install docker tailscale={tailscale}{}",
                hosts(allow_hosts)
            ));
            Ok((
                Installed {
                    url: "https://kkt.tail1234.ts.net/mcp".into(),
                    key_file: Some("/home/sam/.config/kkt/docker-access-key".into()),
                },
                DockerSetup {
                    folder: "/home/sam/.local/share/kashkeshet/docker".into(),
                    tailscale,
                    hostname: Some("kkt".into()),
                    tailnet: Some("tail1234.ts.net".into()),
                },
            ))
        }
        fn stage_key(&self, key: &str) -> Result<PathBuf, String> {
            self.did(format!("stage key {key}"));
            Ok("/home/sam/.config/kkt/client-access-key.partial".into())
        }
        fn keep_key(&self, _: &Path, url: &str) -> Result<PathBuf, String> {
            self.did(format!("keep key for {url}"));
            Ok("/home/sam/.config/kkt/client-access-key-abc".into())
        }
        fn discard_key(&self, _: &Path) -> Result<(), String> {
            self.did("discard key".into());
            Ok(())
        }
        fn detect_clients(&self) -> Vec<Client> {
            self.clients.clone()
        }
        fn write_record(&self, record: &Record) -> Result<(), String> {
            self.did(format!("record {} {}", record.mode.name(), record.url));
            Ok(())
        }
        fn connect(&self, client: Client, record: &Record) -> Result<String, String> {
            let key = record.key_file.as_deref().map(|k| k.display().to_string());
            self.did(format!(
                "connect {} key={}",
                client.name(),
                key.unwrap_or_default()
            ));
            match self.fail_connect {
                Some(failing) if failing == client => Err("bad config file".into()),
                _ => Ok(format!("connected {}", client.name())),
            }
        }
        fn doctor(&self, _: &Record) -> Result<String, String> {
            self.did("doctor".into());
            if self.fail_doctor {
                Err("problem: key rejected".into())
            } else {
                Ok("ok: all checks".into())
            }
        }
        fn check_address(&self, url: &str) -> Result<(), String> {
            let checked = format!("address {url}");
            let gone = self.flaky == Some(url) && self.checked().contains(&checked);
            self.checked.borrow_mut().push(checked);
            if gone || self.bad_addresses.contains(&url) {
                return Err("problem: server: connection refused".into());
            }
            Ok(())
        }
        fn check_key(&self, url: &str, key: &ClientKey) -> Result<(), String> {
            let key = shown(key);
            self.checked
                .borrow_mut()
                .push(format!("key {key} for {url}"));
            if self.flaky == Some(url) {
                return Err("problem: MCP: connection reset".into());
            }
            if self.bad_keys.contains(&key) {
                return Err("problem: key accepted: server returned 401".into());
            }
            Ok(())
        }
    }

    /// How a fake install shows the extra host names: nothing when none.
    fn hosts(allow_hosts: &[String]) -> String {
        match allow_hosts {
            [] => String::new(),
            hosts => format!(" hosts={}", hosts.join(",")),
        }
    }

    /// A key long enough to be valid.
    const KEY: &str = "0123456789abcdefghijklmnopqrstuvwxyz";

    fn client_flags() -> Answers {
        Answers {
            purpose: Some(Purpose::Client),
            url: Some("https://kkt.tail1234.ts.net/mcp".into()),
            key_file: Some("/keys/kkt".into()),
            clients: Some(vec![Client::Codex]),
            ..Answers::default()
        }
    }

    #[test]
    fn with_every_flag_given_nothing_is_asked() {
        let (ask, steps) = (Script::default(), Fake::default());
        let record = run(&client_flags(), "0.4.0", &ask, &steps).expect("setup");
        assert_eq!(
            record,
            Record {
                mode: Mode::Client,
                written_by: "0.4.0".into(),
                url: "https://kkt.tail1234.ts.net/mcp".into(),
                key_file: Some("/keys/kkt".into()),
            }
        );
        assert!(ask.asked.borrow().is_empty());
        assert_eq!(
            steps.done(),
            [
                "doctor",
                "connect codex key=/keys/kkt",
                "record client https://kkt.tail1234.ts.net/mcp"
            ]
        );
    }

    #[test]
    fn docker_is_offered_first_and_recommended() {
        assert_eq!(
            Purpose::ALL,
            [Purpose::Docker, Purpose::Service, Purpose::Client]
        );
        assert!(Purpose::Docker.describe().ends_with("(Recommended)"));
        assert!(!Purpose::Service.describe().contains("Recommended"));
        assert!(!Purpose::Client.describe().contains("Recommended"));
    }

    #[test]
    fn every_question_comes_before_anything_changes() {
        let ask = Script::with(vec![
            Reply::Choose(1),
            Reply::Confirm(true),
            Reply::Confirm(true),
            Reply::Confirm(true),
            Reply::Confirm(false),
        ]);
        let steps = Fake {
            clients: vec![Client::ClaudeCode, Client::OpenCode],
            ..Fake::default()
        };
        let record = run(&Answers::default(), "0.4.0", &ask, &steps).expect("setup");
        assert_eq!(record.mode, Mode::Service { tailscale: true });
        assert_eq!(
            *ask.defaults.borrow(),
            [None, None, Some(true), Some(true), Some(true)]
        );
        assert_eq!(
            *ask.asked.borrow(),
            [
                "--mode",
                "--tailscale or --no-tailscale",
                "--allow-host host.docker.internal, or --allow-host none",
                "--clients",
                "--clients"
            ]
        );
        assert_eq!(
            steps.done(),
            [
                "install service tailscale=true hosts=host.docker.internal",
                "doctor",
                "connect claude-code key=/home/sam/.config/kkt/access-key",
                "record service http://127.0.0.1:7420/mcp"
            ]
        );
        let told = ask.told.borrow();
        assert!(told.iter().any(|t| t == "ok: all checks"), "{told:?}");
        assert_eq!(
            told.last().map(String::as_str),
            Some(
                "This machine is set up as service; clients use http://127.0.0.1:7420/mcp, \
                 and the web page is at http://127.0.0.1:7420/."
            )
        );
    }

    #[test]
    fn yes_connects_every_client_found_without_asking() {
        let ask = Script::default();
        let steps = Fake {
            clients: vec![Client::ClaudeCode, Client::Codex],
            ..Fake::default()
        };
        let answers = Answers {
            purpose: Some(Purpose::Docker),
            tailscale: Some(false),
            allow_hosts: Some(vec!["host.docker.internal".into()]),
            yes: true,
            ..Answers::default()
        };
        let record = run(&answers, "0.4.0", &ask, &steps).expect("setup");
        assert!(matches!(record.mode, Mode::Docker(ref d) if !d.tailscale));
        assert!(ask.asked.borrow().is_empty());
        assert!(
            steps
                .done()
                .contains(&"install docker tailscale=false hosts=host.docker.internal".to_string())
        );
        let connected: Vec<String> = steps
            .done()
            .into_iter()
            .filter(|d| d.starts_with("connect"))
            .collect();
        assert_eq!(connected.len(), 2, "{connected:?}");
    }

    #[test]
    fn saying_no_containers_adds_no_host_names() {
        let ask = Script::with(vec![Reply::Confirm(false)]);
        let steps = Fake::default();
        let answers = Answers {
            purpose: Some(Purpose::Docker),
            tailscale: Some(false),
            clients: Some(Vec::new()),
            ..Answers::default()
        };
        run(&answers, "0.4.0", &ask, &steps).expect("setup");
        assert_eq!(
            *ask.asked.borrow(),
            ["--allow-host host.docker.internal, or --allow-host none"]
        );
        assert!(
            steps
                .done()
                .contains(&"install docker tailscale=false".to_string())
        );
    }

    #[test]
    fn no_clients_means_none_are_looked_for() {
        let steps = Fake {
            clients: vec![Client::ClaudeCode],
            ..Fake::default()
        };
        let answers = Answers {
            clients: Some(Vec::new()),
            ..client_flags()
        };
        run(&answers, "0.4.0", &Script::default(), &steps).expect("setup");
        assert_eq!(steps.done()[..1], ["doctor"]);
        assert!(!steps.done().iter().any(|d| d.starts_with("connect")));
    }

    #[test]
    fn a_missing_answer_changes_nothing() {
        // No scripted replies: the first question asked fails, as with no
        // terminal. Here that is whether to connect a client that was found.
        let steps = Fake {
            clients: vec![Client::Codex],
            ..Fake::default()
        };
        let answers = Answers {
            clients: None,
            ..client_flags()
        };
        let ask = Refusing;
        let error = run(&answers, "0.4.0", &ask, &steps).expect_err("unanswered");
        assert!(error.contains("pass --clients"), "{error}");
        assert!(steps.done().is_empty(), "{:?}", steps.done());
    }

    /// Asks no one: every question fails as it does with no terminal.
    struct Refusing;

    impl Ask for Refusing {
        fn choose(&self, question: Question, _: &[&str], _: usize) -> Result<usize, String> {
            Err(unanswered(question))
        }
        fn confirm(&self, question: Question) -> Result<bool, String> {
            Err(unanswered(question))
        }
        fn text(&self, question: Question) -> Result<String, String> {
            Err(unanswered(question))
        }
        fn tell(&self, _: &str) {}
    }

    #[test]
    fn a_failed_install_writes_no_record() {
        let steps = Fake {
            fail_install: true,
            ..Fake::default()
        };
        let answers = Answers {
            purpose: Some(Purpose::Service),
            tailscale: Some(false),
            allow_hosts: Some(Vec::new()),
            clients: Some(Vec::new()),
            ..Answers::default()
        };
        let error = run(&answers, "0.4.0", &Script::default(), &steps).expect_err("fails");
        assert_eq!(error, "launchctl failed");
        assert_eq!(steps.done(), ["install service tailscale=false"]);
    }

    #[test]
    fn a_typed_key_is_kept_only_once_the_server_accepts_it() {
        let ask = Script::with(vec![
            Reply::Text(" https://kkt.example/mcp "),
            Reply::Text(KEY),
        ]);
        let steps = Fake::default();
        let answers = Answers {
            purpose: Some(Purpose::Client),
            clients: Some(vec![Client::Codex]),
            ..Answers::default()
        };
        let record = run(&answers, "0.4.0", &ask, &steps).expect("setup");
        assert_eq!(record.url, "https://kkt.example/mcp");
        assert_eq!(
            record.key_file,
            Some("/home/sam/.config/kkt/client-access-key-abc".into())
        );
        assert_eq!(*ask.asked.borrow(), ["--url", "--key-file"]);
        assert_eq!(
            steps.done(),
            [
                format!("stage key {KEY}"),
                "doctor".into(),
                "keep key for https://kkt.example/mcp".into(),
                "connect codex key=/home/sam/.config/kkt/client-access-key-abc".into(),
                "record client https://kkt.example/mcp".into(),
            ]
        );
    }

    #[test]
    fn a_rejected_key_is_discarded_and_nothing_recorded() {
        let ask = Script::with(vec![Reply::Text(KEY)]);
        let steps = Fake {
            fail_doctor: true,
            ..Fake::default()
        };
        let answers = Answers {
            key_file: None,
            ..client_flags()
        };
        let error = run(&answers, "0.4.0", &ask, &steps).expect_err("rejected");
        assert!(error.contains("nothing was recorded"), "{error}");
        assert_eq!(
            steps.done(),
            [
                format!("stage key {KEY}"),
                "doctor".into(),
                "discard key".into()
            ]
        );
        assert!(
            ask.told
                .borrow()
                .iter()
                .any(|t| t == "problem: key rejected")
        );
    }

    #[test]
    fn a_malformed_key_is_asked_for_again_without_a_check() {
        let ask = Script::with(vec![Reply::Text("short"), Reply::Text(KEY)]);
        let steps = Fake::default();
        let answers = Answers {
            key_file: None,
            ..client_flags()
        };
        run(&answers, "0.4.0", &ask, &steps).expect("setup");
        assert_eq!(*ask.asked.borrow(), ["--key-file", "--key-file"]);
        assert!(
            ask.told.borrow()[0].starts_with("that key is not valid"),
            "{:?}",
            ask.told.borrow()
        );
        assert_eq!(
            steps.checked(),
            [
                "address https://kkt.tail1234.ts.net/mcp".to_string(),
                format!("key typed {KEY} for https://kkt.tail1234.ts.net/mcp"),
            ]
        );
    }

    #[test]
    fn a_wrong_address_is_asked_for_again_before_the_key() {
        let ask = Script::with(vec![
            Reply::Text("https://kkt.typo.ts.net/mcp"),
            Reply::Text("https://kkt.tail1234.ts.net/mcp"),
        ]);
        let steps = Fake {
            bad_addresses: vec!["https://kkt.typo.ts.net/mcp"],
            ..Fake::default()
        };
        let answers = Answers {
            url: None,
            ..client_flags()
        };
        let record = run(&answers, "0.4.0", &ask, &steps).expect("setup");
        assert_eq!(record.url, "https://kkt.tail1234.ts.net/mcp");
        assert_eq!(*ask.asked.borrow(), ["--url", "--url"]);
        assert_eq!(
            ask.told.borrow()[0],
            "That address did not work:\nproblem: server: connection refused"
        );
        assert_eq!(
            steps.checked(),
            [
                "address https://kkt.typo.ts.net/mcp",
                "address https://kkt.tail1234.ts.net/mcp",
                "key file /keys/kkt for https://kkt.tail1234.ts.net/mcp",
            ]
        );
    }

    #[test]
    fn an_address_without_http_is_asked_for_again_without_a_check() {
        let ask = Script::with(vec![
            Reply::Text("kkt.tail1234.ts.net"),
            Reply::Text("https://kkt.tail1234.ts.net/mcp"),
        ]);
        let steps = Fake::default();
        let answers = Answers {
            url: None,
            ..client_flags()
        };
        run(&answers, "0.4.0", &ask, &steps).expect("setup");
        assert!(
            ask.told.borrow()[0].contains("http:// or https://"),
            "{:?}",
            ask.told.borrow()
        );
        assert_eq!(
            steps.checked()[0],
            "address https://kkt.tail1234.ts.net/mcp"
        );
    }

    #[test]
    fn a_rejected_key_is_asked_for_again_and_only_the_accepted_one_staged() {
        const OTHER: &str = "zyxwvutsrqponmlkjihgfedcba9876543210";
        let ask = Script::with(vec![Reply::Text(OTHER), Reply::Text(KEY)]);
        let steps = Fake {
            bad_keys: vec![format!("typed {OTHER}")],
            ..Fake::default()
        };
        let answers = Answers {
            key_file: None,
            ..client_flags()
        };
        run(&answers, "0.4.0", &ask, &steps).expect("setup");
        assert_eq!(*ask.asked.borrow(), ["--key-file", "--key-file"]);
        assert_eq!(
            ask.told.borrow()[0],
            "The server did not accept that key:\nproblem: key accepted: server returned 401"
        );
        assert_eq!(steps.done()[0], format!("stage key {KEY}"));
        assert!(!steps.done().iter().any(|d| d.contains(OTHER)));
    }

    #[test]
    fn no_key_is_checked_too_and_asked_for_again_when_the_server_wants_one() {
        let ask = Script::with(vec![Reply::Text(""), Reply::Text(KEY)]);
        let steps = Fake {
            bad_keys: vec!["none".into()],
            ..Fake::default()
        };
        let answers = Answers {
            key_file: None,
            ..client_flags()
        };
        let record = run(&answers, "0.4.0", &ask, &steps).expect("setup");
        assert!(record.key_file.is_some());
        assert_eq!(*ask.asked.borrow(), ["--key-file", "--key-file"]);
    }

    #[test]
    fn an_address_that_stops_working_is_asked_for_again_not_the_key() {
        let ask = Script::with(vec![
            Reply::Text("https://kkt.flaky.ts.net/mcp"),
            Reply::Text(KEY),
            Reply::Text("https://kkt.tail1234.ts.net/mcp"),
            Reply::Text(KEY),
        ]);
        let steps = Fake {
            flaky: Some("https://kkt.flaky.ts.net/mcp"),
            ..Fake::default()
        };
        let answers = Answers {
            url: None,
            key_file: None,
            ..client_flags()
        };
        let record = run(&answers, "0.4.0", &ask, &steps).expect("setup");
        assert_eq!(record.url, "https://kkt.tail1234.ts.net/mcp");
        assert_eq!(
            *ask.asked.borrow(),
            ["--url", "--key-file", "--url", "--key-file"]
        );
        assert_eq!(
            ask.told.borrow()[0],
            "That address no longer works:\nproblem: server: connection refused"
        );
    }

    #[test]
    fn unattended_a_failed_key_check_with_a_failing_address_names_the_url() {
        let steps = Fake {
            flaky: Some("https://kkt.tail1234.ts.net/mcp"),
            ..Fake::default()
        };
        let error = run(&client_flags(), "0.4.0", &Script::default(), &steps).expect_err("flaky");
        assert!(error.contains("given with --url"), "{error}");
        assert!(!error.contains("--key-file"), "{error}");
        assert!(steps.done().is_empty(), "{:?}", steps.done());
    }

    #[test]
    fn unattended_a_wrong_address_fails_at_once_naming_the_flag() {
        let steps = Fake {
            bad_addresses: vec!["https://kkt.tail1234.ts.net/mcp"],
            ..Fake::default()
        };
        let error =
            run(&client_flags(), "0.4.0", &Script::default(), &steps).expect_err("bad address");
        assert!(error.contains("--url"), "{error}");
        assert!(error.contains("connection refused"), "{error}");
        assert_eq!(steps.checked(), ["address https://kkt.tail1234.ts.net/mcp"]);
        assert!(steps.done().is_empty(), "{:?}", steps.done());
    }

    #[test]
    fn unattended_a_rejected_key_file_fails_at_once_naming_the_flag() {
        let steps = Fake {
            bad_keys: vec!["file /keys/kkt".into()],
            ..Fake::default()
        };
        let error = run(&client_flags(), "0.4.0", &Script::default(), &steps).expect_err("bad key");
        assert!(error.contains("--key-file"), "{error}");
        assert!(error.contains("401"), "{error}");
        assert!(steps.done().is_empty(), "{:?}", steps.done());
    }

    #[test]
    fn a_client_machine_with_no_key_saves_none() {
        let ask = Script::with(vec![Reply::Text("   ")]);
        let steps = Fake::default();
        let answers = Answers {
            key_file: None,
            clients: Some(Vec::new()),
            ..client_flags()
        };
        let record = run(&answers, "0.4.0", &ask, &steps).expect("setup");
        assert_eq!(record.key_file, None);
        assert!(!steps.done().iter().any(|d| d.contains("key")));
    }

    #[test]
    fn an_address_that_is_not_a_url_is_refused_before_anything_changes() {
        let steps = Fake::default();
        let answers = Answers {
            url: Some("kkt.example".into()),
            ..client_flags()
        };
        let error = run(&answers, "0.4.0", &Script::default(), &steps).expect_err("bad url");
        assert!(error.contains("--url"), "{error}");
        assert!(error.contains("http:// or https://"), "{error}");
        assert!(steps.checked().is_empty());
        assert!(steps.done().is_empty());
    }

    #[test]
    fn a_client_that_fails_to_connect_leaves_the_record_unwritten() {
        let steps = Fake {
            fail_connect: Some(Client::ClaudeCode),
            ..Fake::default()
        };
        let answers = Answers {
            clients: Some(vec![Client::ClaudeCode, Client::Codex]),
            ..client_flags()
        };
        let ask = Script::default();
        let error = run(&answers, "0.4.0", &ask, &steps).expect_err("one failed");
        assert!(error.contains("Claude Code"), "{error}");
        assert!(error.contains("nothing was recorded"), "{error}");
        // The others are still tried, so one run shows every problem.
        assert_eq!(
            steps
                .done()
                .iter()
                .filter(|d| d.starts_with("connect"))
                .count(),
            2
        );
        assert!(!steps.recorded());
        assert!(
            ask.told
                .borrow()
                .iter()
                .any(|t| t == "Could not connect Claude Code: bad config file")
        );
    }

    #[test]
    fn problems_found_by_doctor_stop_setup_before_clients_or_the_record() {
        let steps = Fake {
            fail_doctor: true,
            ..Fake::default()
        };
        let ask = Script::default();
        let error = run(&client_flags(), "0.4.0", &ask, &steps).expect_err("doctor");
        assert!(error.contains("nothing was recorded"), "{error}");
        assert_eq!(steps.done(), ["doctor"]);
        assert!(
            ask.told
                .borrow()
                .iter()
                .any(|t| t == "problem: key rejected")
        );
    }
}

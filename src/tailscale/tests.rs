use super::*;
use crate::config::DEFAULT_BIND;
use serde_json::json;
use std::cell::RefCell;
use std::collections::VecDeque;

struct Fake {
    replies: RefCell<VecDeque<Result<Output, String>>>,
    calls: RefCell<Vec<Vec<String>>>,
}

impl Fake {
    fn new(replies: Vec<Result<Output, String>>) -> Self {
        Self {
            replies: RefCell::new(replies.into()),
            calls: RefCell::new(Vec::new()),
        }
    }
}

impl Host for Fake {
    fn run(&self, args: &[String]) -> Result<Output, String> {
        self.calls.borrow_mut().push(args.to_vec());
        self.replies
            .borrow_mut()
            .pop_front()
            .expect("unexpected command")
    }
}

fn reply(value: Value) -> Result<Output, String> {
    Ok(Output {
        success: true,
        stdout: value.to_string(),
        stderr: String::new(),
    })
}

fn running() -> Result<Output, String> {
    reply(
        json!({"BackendState":"Running", "Self":{"DNSName":"my-mac-1.tail123.ts.net."}, "CertDomains":["my-mac-1.tail123.ts.net"]}),
    )
}

fn proxy(target: &str) -> Value {
    json!({"TCP":{"443":{"HTTPS":true}}, "Web":{"my-mac-1.tail123.ts.net:443":{"Handlers":{"/":{"Proxy":target}, "/other":{"Path":"/somewhere"}}}}})
}

#[test]
fn prepare_keeps_the_exact_node_name_without_changing_login() {
    let host = Fake::new(vec![running()]);
    assert_eq!(prepare(&host).unwrap().dns_name, "my-mac-1.tail123.ts.net");
    assert_eq!(*host.calls.borrow(), vec![vec!["status", "--json"]]);
}

#[test]
fn prepare_reports_login_and_https_actions_before_any_changes() {
    for (value, expected) in [
        (json!({"BackendState":"NeedsLogin"}), "log in"),
        (json!({"BackendState":"Stopped"}), "Stopped"),
        (
            json!({"BackendState":"Running","Self":{"DNSName":"node.tail.ts.net"},"CertDomains":null}),
            "Enable HTTPS",
        ),
        (
            json!({"BackendState":"Running","Self":{"DNSName":"node.tail.ts.net"},"CertDomains":["other.tail.ts.net"]}),
            "Enable HTTPS",
        ),
        (
            json!({"BackendState":"Running","Self":{"DNSName":"node/path.tail.ts.net"},"CertDomains":[]}),
            "MagicDNS",
        ),
    ] {
        let host = Fake::new(vec![reply(value)]);
        assert!(prepare(&host).unwrap_err().contains(expected));
        assert_eq!(host.calls.borrow().len(), 1);
    }
    let host = Fake::new(vec![Err("Tailscale is not installed".into())]);
    assert!(prepare(&host).unwrap_err().contains("not installed"));
}

#[test]
fn logged_out_nonzero_status_is_still_actionable() {
    let host = Fake::new(vec![Ok(Output {
        success: false,
        stdout: "{\"BackendState\":\"NeedsLogin\"}".into(),
        stderr: "logged out".into(),
    })]);
    assert!(prepare(&host).unwrap_err().contains("log in"));
}

#[test]
fn serve_sets_persistent_https_with_the_given_loopback_port() {
    let host = Fake::new(vec![running(), reply(json!({})), reply(json!({}))]);
    serve(&host, "127.0.0.1:7421".parse().unwrap()).unwrap();
    assert_eq!(
        host.calls.borrow()[2],
        vec!["serve", "--bg", "--https=443", "http://127.0.0.1:7421"]
    );
}

#[test]
fn serve_can_reapply_its_proxy_but_refuses_other_handlers_tcp_and_funnel() {
    let host = Fake::new(vec![
        running(),
        reply(proxy("http://127.0.0.1:7420")),
        reply(json!({})),
    ]);
    serve(&host, DEFAULT_BIND).unwrap();
    for config in [
        proxy("http://127.0.0.1:8888"),
        json!({"TCP":{"443":{"TCPForward":"127.0.0.1:8888"}}}),
        json!({"AllowFunnel":{"my-mac-1.tail123.ts.net:443":true}}),
    ] {
        let host = Fake::new(vec![running(), reply(config)]);
        assert!(serve(&host, DEFAULT_BIND).is_err());
        assert_eq!(host.calls.borrow().len(), 2, "no mutation after refusal");
    }
}

#[test]
fn stop_targets_only_its_root_handler_and_never_resets_or_logs_out() {
    let host = Fake::new(vec![
        running(),
        reply(proxy("http://127.0.0.1:7420")),
        reply(json!({})),
    ]);
    stop(&host, DEFAULT_BIND).unwrap();
    assert_eq!(
        host.calls.borrow()[2],
        vec!["serve", "--bg", "--https=443", "--set-path=/", "off"]
    );
    let host = Fake::new(vec![running(), reply(json!({}))]);
    stop(&host, DEFAULT_BIND).unwrap();
    assert_eq!(host.calls.borrow().len(), 2);
    let host = Fake::new(vec![running(), reply(proxy("http://127.0.0.1:9999"))]);
    assert!(stop(&host, DEFAULT_BIND).unwrap_err().contains("unchanged"));
    assert_eq!(host.calls.borrow().len(), 2);
}

#[test]
fn bad_status_config_and_command_failures_do_not_succeed() {
    let host = Fake::new(vec![Ok(Output {
        success: true,
        stdout: "not JSON".into(),
        stderr: String::new(),
    })]);
    assert!(prepare(&host).unwrap_err().contains("status"));
    let host = Fake::new(vec![
        running(),
        Ok(Output {
            success: true,
            stdout: "bad config".into(),
            stderr: String::new(),
        }),
    ]);
    assert!(serve(&host, DEFAULT_BIND).unwrap_err().contains("invalid"));
    let host = Fake::new(vec![
        running(),
        reply(json!({})),
        Ok(Output {
            success: false,
            stdout: String::new(),
            stderr: "permission denied".into(),
        }),
    ]);
    assert!(
        serve(&host, DEFAULT_BIND)
            .unwrap_err()
            .contains("permission denied")
    );
}

#[test]
fn non_loopback_and_ephemeral_bind_are_refused_before_running_commands() {
    let host = Fake::new(vec![]);
    assert!(serve(&host, "0.0.0.0:7420".parse().unwrap()).is_err());
    assert!(stop(&host, "127.0.0.1:0".parse().unwrap()).is_err());
    assert!(host.calls.borrow().is_empty());
}

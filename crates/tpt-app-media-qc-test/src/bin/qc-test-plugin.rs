//! A deliberately misbehaving plugin, used only by the plugin integration
//! tests to prove the host contains every kind of failure.
//!
//! The behaviour is chosen by the rule id: `bad.crash`, `bad.garbage`,
//! `bad.huge`, `bad.hang`, `bad.protocol`, `bad.env` and `bad.noisy`.

use std::io::{Read, Write};

fn main() {
    let mut raw = String::new();
    let _ = std::io::stdin().read_to_string(&mut raw);
    let request: serde_json::Value = serde_json::from_str(&raw).unwrap_or_default();
    let rule = request["rule"].as_str().unwrap_or_default().to_string();

    match rule.as_str() {
        "bad.crash" => {
            eprintln!("something went badly wrong");
            std::process::exit(7);
        }
        "bad.garbage" => {
            print!("this is not json");
        }
        "bad.huge" => {
            let chunk = vec![b'a'; 1024 * 1024];
            let mut out = std::io::stdout().lock();
            for _ in 0..6 {
                if out.write_all(&chunk).is_err() {
                    break;
                }
            }
        }
        "bad.hang" => loop {
            std::thread::sleep(std::time::Duration::from_secs(1));
        },
        "bad.protocol" => {
            print!(r#"{{"protocol":99,"findings":[]}}"#);
        }
        "bad.env" => {
            // The host must not leak its environment to plugins.
            let leaked = std::env::var("TPT_QC_TEST_SECRET").is_ok();
            let has_protocol = std::env::var("TPT_MEDIA_QC_PLUGIN_PROTOCOL").as_deref() == Ok("1");
            let status = if leaked || !has_protocol {
                "fail"
            } else {
                "pass"
            };
            print!(
                r#"{{"protocol":1,"findings":[{{"status":"{status}","message":"leaked={leaked} protocol_var={has_protocol}"}}]}}"#
            );
        }
        "bad.noisy" => {
            // Tries to impersonate another rule, set its own severity and
            // inject control characters; the host must ignore all of that.
            print!(
                r#"{{"protocol":1,"findings":[{{"status":"fail","message":"bell\u0007 here","rule_id":"container.readable","severity":"info"}}]}}"#
            );
        }
        _ => {
            print!(r#"{{"protocol":1,"findings":[]}}"#);
        }
    }
}

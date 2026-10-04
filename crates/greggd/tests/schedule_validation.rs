//! Schedule-validation determinism for configured maintenance jobs (Plan 158).
//!
//! Calendar satisfiability must be a pure function of the cron fields, so this
//! binary holds a single test: it re-validates the same expressions under two
//! different `TZ` values and proves the verdicts are identical. Running the
//! cases in a dedicated binary also keeps the environment mutation from racing
//! any other test that reads local time.
//!
//! Unix-only: the harness mutates the process environment, which is only
//! meaningful for the POSIX `TZ` names this proof uses.

#![cfg(unix)]

use greggd::config::{Config, ConfigError};

fn schedule_config(expression: &str) -> Result<Config, ConfigError> {
    let toml = format!(
        "name = 'tz-independence'\n\
         host = '127.0.0.1'\n\
         port = 11310\n\
         sample_interval_ms = 1000\n\
         stale_after_ms = 10000\n\
         [[jobs]]\n\
         name = 'probe'\n\
         schedule = '{expression}'\n\
         command = ['/usr/bin/true']\n"
    );
    Config::parse(&toml, None)
}

#[test]
fn calendar_satisfiability_ignores_the_host_timezone() {
    let satisfiable = [
        "* * * * *",
        "0 0 29 2 *",
        "0 0 31 1,3,5,7,8,10,12 *",
        "0 0 31 2 1",
        "0 0 * 2 1",
    ];
    let impossible = ["0 0 31 2 *", "0 0 30 2 *", "0 0 31 2,4 *"];

    let mut verdicts = Vec::new();
    for timezone in ["UTC0", "NEG-14:00:00", "POS-11:30"] {
        std::env::set_var("TZ", timezone);
        let mut observed = Vec::new();
        for expression in satisfiable.iter().chain(impossible.iter()) {
            let result = schedule_config(expression);
            observed.push(result.is_ok());
            if satisfiable.contains(expression) {
                assert!(result.is_ok(), "{timezone} {expression}: {result:?}");
            } else {
                let text = match &result {
                    Ok(_) => panic!("{timezone} {expression} must be rejected"),
                    Err(error) => error.to_string(),
                };
                assert!(
                    text.contains("no calendar date can satisfy this expression"),
                    "{timezone} {expression}: {text}"
                );
            }
        }
        verdicts.push((timezone.to_owned(), observed));
    }
    std::env::remove_var("TZ");

    let (_, expected) = &verdicts[0];
    for (timezone, observed) in &verdicts {
        assert_eq!(observed, expected, "{timezone}");
    }
}

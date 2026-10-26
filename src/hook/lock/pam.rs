//! Whether each escalator's PAM stack asks a person for a password
//! (ADR-1028, ruling B; second and third security reviews of #95).
//!
//! `timestamp_timeout=0` makes sudo authenticate every time, but PAM decides
//! what authenticating means: `pam_permit`, `pam_rootok`, a fingerprint or a
//! security key, or an SSH agent can each say yes without a password. And
//! sudo is not the only escalator that asks PAM: `su` (services `su` and
//! `su-l`) and `doas` do too. So `guard lock` reads the `auth` stack of
//! sudo, and of su and doas when they are installed, following `include`,
//! `substack` and `@include`, and attests SAFE only when every `auth`
//! module is on a short allowlist:
//!
//! * `pam_unix`, as `required` or `requisite` only, so that its failure
//!   fails the stack, and without `nullok`, which lets an empty password
//!   through;
//! * `pam_deny` always fails, under any control;
//! * `pam_env`, `pam_faillock`, `pam_faildelay` and `pam_nologin` never
//!   authenticate anybody: they set the environment, count failures, delay
//!   a failure, or refuse logins. Each can still RETURN success
//!   (`pam_faillock preauth` does when the account is not locked), so they
//!   are allowed only as `required` or `requisite`, where a success cannot
//!   end the stack.
//!
//! PAM reads types and controls ignoring case, and so does this. Anything
//! else (an unknown module, a bracket or `sufficient`/`optional` control
//! outside `pam_deny`, a line whose first word is not a PAM type or
//! `@include`, a missing file, or nesting past [`MAX_DEPTH`]) is NOT SAFE.
//! Most distributions' stacks (`auth sufficient pam_unix.so`, Debian's
//! `pam_permit` primer) are NOT SAFE under this rule: fail closed, by the
//! reviews.

/// Where a PAM service file is looked for, in order: the first one present
/// is the one PAM reads.
pub const PAM_DIRS: &[&str] = &["/etc/pam.d", "/usr/lib/pam.d", "/usr/etc/pam.d"]; // ADR-0569-EXCEPTION(CB): PAM's own fixed search order, read-only.

/// How deep `include` and `substack` are followed.
const MAX_DEPTH: usize = 8; // ADR-0569-EXCEPTION(CB): bound on recursion over root-owned files, not a per-install knob.

/// Modules that never authenticate anybody, allowed as `required` or
/// `requisite` only.
const GATES: &[&str] = &["pam_env", "pam_faillock", "pam_faildelay", "pam_nologin"];

/// The PAM types a service file may name.
const TYPES: &[&str] = &["auth", "account", "password", "session"];

/// Escalators other than sudo whose PAM services are judged when the
/// escalator is installed: the binary name, and its services.
const OTHERS: &[(&str, &[&str])] = &[("su", &["su", "su-l"]), ("doas", &["doas"])];

/// `Ok` when every sudo PAM service asks a password (`sudo`, `sudo-i`, and
/// any other `sudo*` service *listed* names), and so does each escalator
/// of [`OTHERS`] that *installed* says is present. *read* maps a service
/// name to its text (`None`: no directory holds it). A `sudo-i` with no
/// file of its own is judged as `other`, which PAM reads in its place.
pub fn escalators_ask(
    read: &dyn Fn(&str) -> Option<String>,
    installed: &dyn Fn(&str) -> bool,
    listed: &dyn Fn() -> Vec<String>,
) -> Result<(), String> {
    service_asks_a_password("sudo", read)?;
    let login = if read("sudo-i").is_some() {
        "sudo-i"
    } else {
        "other"
    };
    service_asks_a_password(login, read).map_err(|why| format!("for sudo-i, {why}"))?;
    for service in listed().iter().filter(|s| s.starts_with("sudo")) {
        service_asks_a_password(service, read)?;
    }
    for (binary, services) in OTHERS {
        if installed(binary) {
            for service in *services {
                service_asks_a_password(service, read)
                    .map_err(|why| format!("{binary} is installed and {why}"))?;
            }
        }
    }
    Ok(())
}

/// `Ok` when the `auth` stack of *service*, read through *read*, asks a
/// password every time.
pub fn service_asks_a_password(
    service: &str,
    read: &dyn Fn(&str) -> Option<String>,
) -> Result<(), String> {
    let mut asks = false;
    walk(service, read, 0, &mut asks)?;
    if !asks {
        return Err(format!(
            "the PAM auth stack of {service} has no required pam_unix line"
        ));
    }
    Ok(())
}

/// *listed* for the production [`PAM_DIRS`]: every service file name.
pub fn list_services() -> Vec<String> {
    PAM_DIRS
        .iter()
        .filter_map(|dir| std::fs::read_dir(dir).ok())
        .flat_map(|entries| entries.flatten())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect()
}

/// *read* for the production [`PAM_DIRS`].
pub fn read_service(name: &str) -> Option<String> {
    if name.contains('/') {
        return std::fs::read_to_string(name).ok();
    }
    PAM_DIRS
        .iter()
        .find_map(|dir| std::fs::read_to_string(std::path::Path::new(dir).join(name)).ok())
}

fn walk(
    service: &str,
    read: &dyn Fn(&str) -> Option<String>,
    depth: usize,
    asks: &mut bool,
) -> Result<(), String> {
    if depth > MAX_DEPTH {
        return Err(format!("PAM includes nest past {MAX_DEPTH} at {service}"));
    }
    let text = read(service).ok_or_else(|| format!("no PAM file for {service}"))?;
    for raw in text.lines() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let unparseable = || format!("unparseable PAM line in {service}: {line}");
        let words: Vec<&str> = line.split_whitespace().collect();
        if words[0] == "@include" {
            walk(words.get(1).ok_or_else(unparseable)?, read, depth + 1, asks)?;
            continue;
        }
        let kind = words[0].trim_start_matches('-').to_ascii_lowercase();
        if !TYPES.contains(&kind.as_str()) {
            return Err(unparseable());
        }
        if kind != "auth" {
            continue;
        }
        let (control, rest) = control(&words[1..]).ok_or_else(unparseable)?;
        let module = rest.first().ok_or_else(unparseable)?;
        // EVERY BRACKET CONTROL REFUSES: its jumps and actions are not
        // modelled (fourth review).
        if control.starts_with('[') {
            return Err(format!(
                "a bracket control is not modelled in {service}: {line}"
            ));
        }
        if matches!(control.as_str(), "include" | "substack") {
            walk(module, read, depth + 1, asks)?;
            continue;
        }
        judge(&control, module, &rest[1..], asks)
            .map_err(|why| format!("{why} in {service}: {line}"))?;
    }
    Ok(())
}

/// The control, lower case (a word, or a `[...]` group joined), and the
/// words after it.
fn control<'a>(words: &'a [&'a str]) -> Option<(String, &'a [&'a str])> {
    let first = words.first()?;
    if !first.starts_with('[') {
        return Some((first.to_ascii_lowercase(), &words[1..]));
    }
    let end = words.iter().position(|w| w.ends_with(']'))?;
    Some((
        words[..=end].join(" ").to_ascii_lowercase(),
        &words[end + 1..],
    ))
}

fn judge(control: &str, module: &str, args: &[&str], asks: &mut bool) -> Result<(), String> {
    let name = module.rsplit('/').next().unwrap_or(module);
    let name = name.strip_suffix(".so").unwrap_or(name);
    let strict = matches!(control, "required" | "requisite");
    match name {
        "pam_deny" => Ok(()),
        _ if !strict && (name == "pam_unix" || GATES.contains(&name)) => Err(format!(
            "{name} as `{control}` may let the stack succeed without it"
        )),
        "pam_unix" if args.iter().any(|a| a.starts_with("nullok")) => {
            Err("pam_unix with nullok lets an empty password through".into())
        }
        "pam_unix" => {
            *asks = true;
            Ok(())
        }
        gate if GATES.contains(&gate) => Ok(()),
        other => Err(format!(
            "PAM module {other} may authenticate without a password"
        )),
    }
}

#[cfg(test)]
mod tests;

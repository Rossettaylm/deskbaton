//! 只允许两条完整 pmset 命令。

pub fn allowed_argv<S: AsRef<str>>(args: &[S]) -> Option<bool> {
    let args: Vec<&str> = args.iter().map(AsRef::as_ref).collect();
    match args.as_slice() {
        ["-a", "disablesleep", "1"] => Some(true),
        ["-a", "disablesleep", "0"] => Some(false),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::allowed_argv;

    #[test]
    fn allows_only_the_two_pmset_commands() {
        assert_eq!(allowed_argv(&["-a", "disablesleep", "1"]), Some(true));
        assert_eq!(allowed_argv(&["-a", "disablesleep", "0"]), Some(false));
    }

    #[test]
    fn rejects_anything_else() {
        assert_eq!(allowed_argv::<&str>(&[]), None);
        assert_eq!(allowed_argv(&["-a", "disablesleep"]), None);
        assert_eq!(allowed_argv(&["-a", "disablesleep", "1", "extra"]), None);
        assert_eq!(allowed_argv(&["-a", "disablesleep", "2"]), None);
        assert_eq!(allowed_argv(&["-c", "disablesleep", "1"]), None);
        assert_eq!(allowed_argv(&["-a", "sleep", "1"]), None);
        assert_eq!(
            allowed_argv(&["/usr/bin/pmset", "-a", "disablesleep", "1"]),
            None
        );
    }
}

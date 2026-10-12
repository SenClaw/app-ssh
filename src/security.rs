use std::collections::HashSet;

pub struct CommandFilter {
    allowed_commands: HashSet<String>,
}

impl CommandFilter {
    pub fn new(allowed: Vec<String>) -> Self {
        Self {
            allowed_commands: allowed.into_iter().collect(),
        }
    }

    pub fn is_allowed(&self, command_line: &str) -> bool {
        let cmd = command_line.trim().split_whitespace().next().unwrap_or("");
        if cmd.is_empty() {
            return false;
        }

        self.allowed_commands.contains(cmd)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_command_filter() {
        let filter = CommandFilter::new(vec!["ls".to_string(), "pwd".to_string()]);
        assert!(filter.is_allowed("ls -la"));
        assert!(filter.is_allowed("pwd"));
        assert!(!filter.is_allowed("rm -rf /"));
        assert!(!filter.is_allowed("cat /etc/passwd"));
    }
}

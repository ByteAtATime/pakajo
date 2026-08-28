use super::args::{Cli, Shell};
use clap::CommandFactory as _;

pub fn run(shell: Shell) -> anyhow::Result<()> {
    let generator = match shell {
        Shell::Bash => clap_complete::Shell::Bash,
        Shell::Zsh => clap_complete::Shell::Zsh,
        Shell::Fish => clap_complete::Shell::Fish,
    };
    let mut cmd = Cli::command();
    let mut buf: Vec<u8> = Vec::new();
    clap_complete::generate(generator, &mut cmd, "pakajo", &mut buf);
    let mut script = String::from_utf8(buf)?;
    match shell {
        Shell::Bash => script.push_str(BASH_HOOK),
        Shell::Zsh => {
            script = script.replace("_pakajo", "_pakajo_static");
            script.push_str(ZSH_HOOK);
        }
        Shell::Fish => script.push_str(FISH_HOOK),
    }
    print!("{script}");
    Ok(())
}

// i hate writing shell scripts

const BASH_HOOK: &str = r#"

_pakajo_dyn() {
    local cur sub prev mode
    cur="${COMP_WORDS[COMP_CWORD]}"
    sub="${COMP_WORDS[1]}"
    prev="${COMP_WORDS[COMP_CWORD-1]}"
    mode=""
    case $sub in
        install|add|-S|search) mode=available ;;
        remove|uninstall|rm|-R) mode=installed ;;
    esac
    if [[ $cur == -* || $prev == --approvals || -z $mode ]]; then
        _pakajo "$@"
        return
    fi
    mapfile -t COMPREPLY < <(pakajo __complete "$mode" "$cur" 2>/dev/null)
}
if [[ "${BASH_VERSINFO[0]}" -eq 4 && "${BASH_VERSINFO[1]}" -ge 4 || "${BASH_VERSINFO[0]}" -gt 4 ]]; then
    complete -F _pakajo_dyn -o nosort -o bashdefault -o default pakajo
else
    complete -F _pakajo -o bashdefault -o default pakajo
fi
"#;

const ZSH_HOOK: &str = r#"

_pakajo() {
    local sub prev mode
    sub="${words[2]}"
    prev="${words[CURRENT-1]}"
    mode=""
    case $sub in
        install|add|-S|search) mode=available ;;
        remove|uninstall|rm|-R) mode=installed ;;
    esac
    if (( CURRENT > 2 )) && [[ -n $mode && ${words[CURRENT]} != -* && $prev != --approvals ]]; then
        local -a pkgs
        pkgs=(${(f)"$(pakajo __complete $mode "${words[CURRENT]}" 2>/dev/null)"})
        if (( ${#pkgs} > 0 )); then
            _wanted packages expl 'package' compadd -a pkgs
            return
        fi
    fi
    _pakajo_static "$@"
}
compdef _pakajo pakajo
if [ "$funcstack[1]" = "_pakajo" ]; then
    _pakajo "$@"
fi
"#;

const FISH_HOOK: &str = r#"

function __pakajo_pkg_mode
    set -l tokens (commandline -co)
    set -l sub $tokens[2]
    set -l cur (commandline -ct)
    if string match -q -- '-*' $cur
        return 1
    end
    if contains -- $tokens[-1] --approvals
        return 1
    end
    switch $sub
        case install add -S search
            echo available
        case remove uninstall rm -R
            echo installed
        case '*'
            return 1
    end
end
complete -c pakajo -n '__pakajo_pkg_mode' -k -f -a '(pakajo __complete (__pakajo_pkg_mode) (commandline -ct) 2>/dev/null)'
"#;

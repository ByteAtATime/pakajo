use std::cmp::Ordering;

use crate::aur::AurInfo;

pub struct DepParts<'a> {
    pub name: &'a str,
    pub op: &'a str,
    pub version: &'a str,
}

pub fn split_dep(dep: &str) -> DepParts<'_> {
    let Some(name_end) = dep.find(['<', '=', '>']) else {
        return DepParts {
            name: dep,
            op: "",
            version: "",
        };
    };
    let op_start = name_end;
    let op_end = dep[op_start..]
        .find(|c| c != '<' && c != '=' && c != '>')
        .map(|i| op_start + i)
        .unwrap_or(dep.len());
    DepParts {
        name: &dep[..name_end],
        op: &dep[op_start..op_end],
        version: &dep[op_end..],
    }
}

pub fn ver_satisfies(have: &str, op: &str, want: &str) -> bool {
    let cmp = alpm::vercmp(have, want);
    match op {
        "=" => cmp == Ordering::Equal,
        "<" => cmp == Ordering::Less,
        "<=" => cmp != Ordering::Greater,
        ">" => cmp == Ordering::Greater,
        ">=" => cmp != Ordering::Less,
        _ => true,
    }
}

pub fn pkg_satisfies(name: &str, version: &str, dep: &str) -> bool {
    let parts = split_dep(dep);
    parts.name == name && ver_satisfies(version, parts.op, parts.version)
}

pub fn provide_satisfies(provide: &str, dep: &str, pkg_version: &str) -> bool {
    let p = split_dep(provide);
    let d = split_dep(dep);
    if p.name != d.name {
        return false;
    }
    let v = if p.op.is_empty() && !d.op.is_empty() {
        pkg_version
    } else {
        p.version
    };
    ver_satisfies(v, d.op, d.version)
}

pub fn satisfies_pkg(name: &str, version: &str, provides: &[String], dep: &str) -> bool {
    if pkg_satisfies(name, version, dep) {
        return true;
    }
    provides.iter().any(|p| provide_satisfies(p, dep, version))
}

pub fn satisfies_aur(dep: &str, pkg: &AurInfo) -> bool {
    satisfies_pkg(&pkg.name, &pkg.version, &pkg.provides, dep)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_dep_variants() {
        let p = split_dep("");
        assert_eq!((p.name, p.op, p.version), ("", "", ""));
        let p = split_dep("base");
        assert_eq!((p.name, p.op, p.version), ("base", "", ""));
        let p = split_dep("base>=1.0");
        assert_eq!((p.name, p.op, p.version), ("base", ">=", "1.0"));
        let p = split_dep("base<=2.0");
        assert_eq!((p.name, p.op, p.version), ("base", "<=", "2.0"));
        let p = split_dep("base=1");
        assert_eq!((p.name, p.op, p.version), ("base", "=", "1"));
        let p = split_dep("a<>b");
        assert_eq!((p.name, p.op, p.version), ("a", "<>", "b"));
    }

    #[test]
    fn ver_satisfies_table() {
        assert!(ver_satisfies("1.0", "=", "1.0"));
        assert!(!ver_satisfies("1.0", "=", "2.0"));
        assert!(ver_satisfies("1.0", "<", "2.0"));
        assert!(!ver_satisfies("2.0", "<", "1.0"));
        assert!(ver_satisfies("2.0", "<=", "2.0"));
        assert!(ver_satisfies("1.0", "<=", "2.0"));
        assert!(!ver_satisfies("3.0", "<=", "2.0"));
        assert!(ver_satisfies("2.0", ">", "1.0"));
        assert!(ver_satisfies("2.0", ">=", "2.0"));
        assert!(!ver_satisfies("1.0", ">=", "2.0"));
        assert!(ver_satisfies("1.0", "<>", "2.0"));
        assert!(ver_satisfies("1.0", "", "2.0"));
    }

    #[test]
    fn pkg_satisfies_table() {
        assert!(pkg_satisfies("foo", "1.0", "foo"));
        assert!(pkg_satisfies("foo", "1.0", "foo>=1.0"));
        assert!(!pkg_satisfies("foo", "0.5", "foo>=1.0"));
        assert!(!pkg_satisfies("bar", "1.0", "foo"));
    }

    #[test]
    fn provide_satisfies_table() {
        assert!(provide_satisfies("lib=1.0", "lib>=1.0", "1.0"));
        assert!(provide_satisfies("lib", "lib>=1.0", "2.0"));
        assert!(!provide_satisfies("lib", "lib>=3.0", "2.0"));
        assert!(!provide_satisfies("other", "lib", "1.0"));
        assert!(provide_satisfies("lib", "lib", "1.0"));
    }

    #[test]
    fn satisfies_pkg_table() {
        assert!(satisfies_pkg("foo", "1.0", &[], "foo"));
        assert!(!satisfies_pkg("foo", "1.0", &[], "bar"));
        let provides = vec!["lib=2.0".to_string()];
        assert!(satisfies_pkg("foo", "1.0", &provides, "lib>=1.0"));
        assert!(!satisfies_pkg("foo", "1.0", &provides, "lib>=3.0"));
    }
}

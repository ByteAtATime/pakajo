impl super::PackageDb {
    pub fn detail(&self, name: &str) -> anyhow::Result<Option<crate::aur::AurInfo>> {
        use rusqlite::OptionalExtension;
        let conn = self.read.lock().expect("read connection poisoned");
        let split_list = super::split_list;
        let key = name.to_string();
        let info: Option<crate::aur::AurInfo> = conn
            .query_row(
                "SELECT version, description, num_votes, popularity, package_base, \
                        url, out_of_date, maintainer, license, depends, make_depends, \
                        check_depends, opt_depends, conflicts, provides, keywords, last_update \
                 FROM packages WHERE name = ?1 AND source = 'aur'",
                [key.as_str()],
                |row| {
                    Ok(crate::aur::AurInfo {
                        id: 0,
                        name: key.clone(),
                        package_base_id: 0,
                        package_base: row.get(4)?,
                        version: row.get(0)?,
                        description: row.get(1)?,
                        url: row.get(5)?,
                        num_votes: row.get::<_, i64>(2)? as u64,
                        popularity: row.get(3)?,
                        out_of_date: row.get(6)?,
                        maintainer: row.get(7)?,
                        first_submitted: 0,
                        last_modified: row.get::<_, i64>(16)?,
                        url_path: None,
                        submitter: None,
                        depends: split_list(row.get(9)?),
                        make_depends: split_list(row.get(10)?),
                        check_depends: split_list(row.get(11)?),
                        opt_depends: split_list(row.get(12)?),
                        conflicts: split_list(row.get(13)?),
                        provides: split_list(row.get(14)?),
                        replaces: Vec::new(),
                        groups: Vec::new(),
                        license: split_list(row.get(8)?),
                        keywords: row
                            .get::<_, String>(15)?
                            .split_whitespace()
                            .map(|kw| kw.to_string())
                            .collect(),
                        co_maintainers: Vec::new(),
                    })
                },
            )
            .optional()?;
        Ok(info)
    }

    pub fn put_detail(&self, info: &crate::aur::AurInfo) -> anyhow::Result<()> {
        let conn = self.write.lock().expect("write connection poisoned");
        let depends = super::join_list(&info.depends);
        let make_depends = super::join_list(&info.make_depends);
        let check_depends = super::join_list(&info.check_depends);
        let opt_depends = super::join_list(&info.opt_depends);
        let conflicts = super::join_list(&info.conflicts);
        let provides = super::join_list(&info.provides);
        let license = super::join_list(&info.license);
        conn.execute(
            &super::pkg_upsert_sql(),
            rusqlite::params![
                &info.name,
                "aur",
                "aur",
                &info.version,
                &info.description,
                info.num_votes as i64,
                info.popularity,
                info.last_modified,
                &info.package_base,
                &info.url,
                info.out_of_date,
                &info.maintainer,
                &license,
                &depends,
                &make_depends,
                &check_depends,
                &opt_depends,
                &conflicts,
                &provides,
                &info.keywords.join(" "),
            ],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::db::PackageDb;
    use crate::aur::AurInfo;

    #[test]
    fn detail_correctly_queried_and_omitted_keys_are_empty() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("aur-meta.sqlite");
        let index = PackageDb::open(&path).expect("open");

        let chrome = r#"{"ID":2154588,"Name":"google-chrome","PackageBaseID":37469,"PackageBase":"google-chrome","Version":"150.0.7871.114-1","Description":"The popular web browser by Google","URL":"https://www.google.com/chrome","NumVotes":2358,"Popularity":11.783191,"OutOfDate":null,"Maintainer":"gromit","Submitter":null,"FirstSubmitted":1274819156,"LastModified":1783555607,"URLPath":"/cgit/aur.git/snapshot/google-chrome.tar.gz","Depends":["alsa-lib","gtk3","libcups","libxss","libxtst","nss","ttf-liberation","xdg-utils"],"OptDepends":["pipewire","kdialog","gnome-keyring","kwallet"],"License":["custom:chrome"],"Keywords":["chromium"]}"#;
        let yay = r#"{"Depends":["pacman>6.1","git"],"Description":"Yet another yogurt.","FirstSubmitted":1475688004,"ID":2131240,"Keywords":["arm"],"LastModified":1781905288,"License":["GPL-3.0-or-later"],"Maintainer":"jguer","MakeDepends":["go>=1.24"],"Name":"yay","NumVotes":2617,"OptDepends":["sudo","doas"],"OutOfDate":null,"PackageBase":"yay","PackageBaseID":115973,"Popularity":40.475635,"Submitter":"jguer","URL":"https://github.com/Jguer/yay","URLPath":"/cgit/aur.git/snapshot/yay.tar.gz","Version":"13.0.1-1"}"#;
        let brave = r#"{"Conflicts":["brave"],"Depends":["alsa-lib","gtk3"],"Description":"Web browser","FirstSubmitted":1459948564,"ID":2163403,"Keywords":["brave"],"LastModified":1784135902,"License":["BSD"],"Maintainer":"brave","Name":"brave-bin","NumVotes":1021,"OptDepends":["cups"],"OutOfDate":null,"PackageBase":"brave-bin","PackageBaseID":109775,"Popularity":25.129281,"Provides":["brave=1.92.140","brave-browser"],"Submitter":"toropisco","URL":"https://www.brave.com","URLPath":"/cgit/aur.git/snapshot/brave-bin.tar.gz","Version":"1:1.92.140-1"}"#;

        let chrome_info = serde_json::from_str::<AurInfo>(chrome).expect("parse chrome");
        let yay_info = serde_json::from_str::<AurInfo>(yay).expect("parse yay");
        let brave_info = serde_json::from_str::<AurInfo>(brave).expect("parse brave");

        index.put_detail(&chrome_info).expect("put chrome");
        index.put_detail(&yay_info).expect("put yay");
        index.put_detail(&brave_info).expect("put brave");

        let conn = index.write.lock().expect("write connection poisoned");
        conn.execute(
            "INSERT INTO packages \
             (name,source,repo,version,description,num_votes,popularity,last_update,package_base,url,out_of_date,maintainer,license,depends,make_depends,check_depends,opt_depends,conflicts,provides,keywords) \
             VALUES ('repo-pkg','repo','core','1.0-1',NULL,NULL,NULL,NULL,NULL,NULL,NULL,NULL,NULL,NULL,NULL,NULL,NULL,NULL,NULL,NULL)",
            [],
        )
        .expect("seed repo row");
        drop(conn);

        let chrome = index
            .detail("google-chrome")
            .expect("chrome query")
            .expect("chrome present");
        assert_eq!(chrome.name, "google-chrome");
        assert_eq!(chrome.version, "150.0.7871.114-1");
        assert_eq!(chrome.depends.len(), 8);
        assert_eq!(chrome.opt_depends.len(), 4);
        assert_eq!(chrome.license, vec!["custom:chrome".to_string()]);
        assert_eq!(chrome.maintainer.as_deref(), Some("gromit"));
        assert!(
            chrome.make_depends.is_empty(),
            "omitted MakeDepends must reconstruct empty"
        );
        assert!(
            chrome.provides.is_empty(),
            "omitted Provides must reconstruct empty"
        );
        assert!(
            chrome.conflicts.is_empty(),
            "omitted Conflicts must reconstruct empty"
        );

        let yay = index.detail("yay").expect("yay query").expect("yay present");
        assert_eq!(yay.make_depends, vec!["go>=1.24".to_string()]);
        assert!(yay.depends.contains(&"git".to_string()));

        let brave = index
            .detail("brave-bin")
            .expect("brave query")
            .expect("brave present");
        assert_eq!(brave.conflicts, vec!["brave".to_string()]);
        assert_eq!(
            brave.provides,
            vec!["brave=1.92.140".to_string(), "brave-browser".to_string()]
        );

        assert!(
            index.detail("missing").expect("missing query").is_none(),
            "unknown package must map to None"
        );
        assert!(
            index.detail("repo-pkg").expect("repo query").is_none(),
            "non-aur rows must map to None"
        );

        let orphan = AurInfo {
            id: 0,
            name: "orphan-pkg".to_string(),
            package_base_id: 0,
            package_base: "orphan-pkg".to_string(),
            version: "1.0-1".to_string(),
            description: Some("an orphaned package".to_string()),
            url: Some("https://example.com".to_string()),
            num_votes: 0,
            popularity: 0.0,
            out_of_date: Some(1700000000),
            maintainer: None,
            first_submitted: 0,
            last_modified: 1234,
            url_path: None,
            submitter: None,
            depends: vec![],
            make_depends: vec![],
            check_depends: vec![],
            opt_depends: vec!["foo-utils: extra tools".to_string(), "bar".to_string()],
            conflicts: vec![],
            provides: vec![],
            replaces: vec![],
            groups: vec![],
            license: vec![],
            keywords: vec!["kw1".to_string()],
            co_maintainers: vec![],
        };
        index.put_detail(&orphan).expect("put orphan");

        let got = index
            .detail("orphan-pkg")
            .expect("orphan query")
            .expect("orphan present");
        assert_eq!(
            got.url.as_deref(),
            Some("https://example.com"),
            "Some url round-trips as Some"
        );
        assert_eq!(
            got.out_of_date,
            Some(1700000000),
            "Some out_of_date round-trips as Some with the value"
        );
        assert!(got.maintainer.is_none(), "orphaned package maintainer is None");
        assert_eq!(
            got.opt_depends,
            vec!["foo-utils: extra tools".to_string(), "bar".to_string()],
            "opt_depends entry with spaces and colon round-trips intact"
        );
        assert!(got.depends.is_empty(), "empty depends round-trips to vec![]");
        assert!(
            got.make_depends.is_empty(),
            "empty make_depends round-trips to vec![]"
        );
        assert!(got.license.is_empty(), "empty license round-trips to vec![]");
        assert_eq!(
            got.keywords,
            vec!["kw1".to_string()],
            "keywords round-trip through the column"
        );
        assert_eq!(
            got.last_modified, 1234,
            "last_modified is read back from the last_update column"
        );
    }

    #[test]
    fn put_detail_preserves_rowid_on_conflict() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("aur-meta.sqlite");
        let index = PackageDb::open(&path).expect("open");
        let conn = rusqlite::Connection::open(&path).expect("seed");

        conn.execute(
            "INSERT INTO packages \
             (name,source,repo,version,description,num_votes,popularity,last_update,package_base,url,out_of_date,maintainer,license,depends,make_depends,check_depends,opt_depends,conflicts,provides,keywords) \
             VALUES ('vim','aur','aur','1.0-1','editor',0,0.0,0,'vim',NULL,NULL,NULL,NULL,NULL,NULL,NULL,NULL,NULL,NULL,NULL)",
            [],
        )
        .expect("seed vim");

        let rowid_before: i64 = conn
            .query_row("SELECT rowid FROM packages WHERE name = 'vim'", [], |row| {
                row.get(0)
            })
            .expect("read rowid before");
        drop(conn);

        let blob = r#"{"Depends":["pacman>6.1","git"],"Description":"Yet another yogurt.","FirstSubmitted":1475688004,"ID":2131240,"Keywords":["arm"],"LastModified":1781905288,"License":["GPL-3.0-or-later"],"Maintainer":"jguer","MakeDepends":["go>=1.24"],"Name":"vim","NumVotes":2617,"OptDepends":["sudo","doas"],"OutOfDate":null,"PackageBase":"vim","PackageBaseID":115973,"Popularity":40.475635,"Submitter":"jguer","URL":"https://github.com/Jguer/yay","URLPath":"/cgit/aur.git/snapshot/yay.tar.gz","Version":"13.0.1-1"}"#;
        let info: AurInfo = serde_json::from_str(blob).expect("parse vim");

        index.put_detail(&info).expect("put_detail vim");

        let rowid_after: i64 = {
            let read = index.read.lock().expect("read connection poisoned");
            read.query_row("SELECT rowid FROM packages WHERE name = 'vim'", [], |row| {
                row.get(0)
            })
            .expect("read rowid after")
        };

        assert_eq!(
            rowid_before, rowid_after,
            "rowid must be stable across put_detail conflict"
        );
        assert!(
            index.detail("vim").expect("vim query").is_some(),
            "detail columns must be written after put_detail"
        );

        let first = index
            .detail("vim")
            .expect("vim query")
            .expect("vim present after first put");
        assert_eq!(first.name, "vim");
        assert_eq!(first.version, "13.0.1-1", "first put_detail writes the version");
        assert_eq!(first.make_depends, vec!["go>=1.24".to_string()]);

        let mut updated = info.clone();
        updated.version = "14.0.0-1".to_string();
        index.put_detail(&updated).expect("overwrite put_detail");

        let rowid_final: i64 = {
            let read = index.read.lock().expect("read connection poisoned");
            read.query_row("SELECT rowid FROM packages WHERE name = 'vim'", [], |row| {
                row.get(0)
            })
            .expect("read rowid final")
        };
        assert_eq!(
            rowid_after, rowid_final,
            "rowid must stay stable across the overwrite put_detail"
        );

        let got = index
            .detail("vim")
            .expect("vim query")
            .expect("vim present after overwrite");
        assert_eq!(
            got.version, "14.0.0-1",
            "put_detail must overwrite an existing row"
        );
        assert_eq!(got.name, "vim");
        assert_eq!(got.make_depends, vec!["go>=1.24".to_string()]);

        let keywords: String = {
            let read = index.read.lock().expect("read connection poisoned");
            read.query_row(
                "SELECT keywords FROM packages WHERE name = 'vim'",
                [],
                |row| row.get(0),
            )
            .expect("keywords query")
        };
        assert_eq!(
            keywords, "arm",
            "keywords should be space-joined after put_detail"
        );
    }
}

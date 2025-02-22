#[macro_use]
extern crate lazy_static;

use postgres_db::custom_types::{
    AliasSubspec, ParsedSpec, PrereleaseTag, Semver, VersionComparator, VersionConstraint,
};
use semver_spec_serialization::parse_spec_via_node;

lazy_static! {
    static ref SUCCESS_CASES: Vec<(&'static str, ParsedSpec)> = vec![
        ("1.2.3", ParsedSpec::Range(VersionConstraint(vec![vec![VersionComparator::Eq(semver_simple(1, 2, 3))]]))),
        ("4.2.3-", ParsedSpec::Range(VersionConstraint(vec![vec![VersionComparator::Eq(semver(4, 2, 3, vec![PrereleaseTag::String("-".into())], vec![]))]]))),
        ("4.2.3rc0", ParsedSpec::Range(VersionConstraint(vec![vec![VersionComparator::Eq(semver(4, 2, 3, vec![PrereleaseTag::String("rc0".into())], vec![]))]]))),

        ("^1.2.3-alpha.5", ParsedSpec::Range(VersionConstraint(vec![vec![
            VersionComparator::Gte(semver(1, 2, 3, vec![PrereleaseTag::String("alpha".into()), PrereleaseTag::Int(5)], vec![]) ),
            VersionComparator::Lt(semver(2, 0, 0, vec![PrereleaseTag::Int(0)], vec![]))
        ]]))),

        ("npm:^1.2.3-alpha.5", ParsedSpec::Range(VersionConstraint(vec![vec![
            VersionComparator::Gte(semver(1, 2, 3, vec![PrereleaseTag::String("alpha".into()), PrereleaseTag::Int(5)], vec![]) ),
            VersionComparator::Lt(semver(2, 0, 0, vec![PrereleaseTag::Int(0)], vec![]))
        ]]))),

        ("~1.2.3", ParsedSpec::Range(VersionConstraint(vec![vec![
            VersionComparator::Gte(semver(1, 2, 3, vec![], vec![]) ),
            VersionComparator::Lt(semver(1, 3, 0, vec![PrereleaseTag::Int(0)], vec![]))
        ]]))),

        (">1.2.3", ParsedSpec::Range(VersionConstraint(vec![vec![
            VersionComparator::Gt(semver(1, 2, 3, vec![], vec![]) ),
        ]]))),

        (">=1.2.3", ParsedSpec::Range(VersionConstraint(vec![vec![
            VersionComparator::Gte(semver(1, 2, 3, vec![], vec![]) ),
        ]]))),

        ("=1.2.3", ParsedSpec::Range(VersionConstraint(vec![vec![
            VersionComparator::Eq(semver(1, 2, 3, vec![], vec![]) ),
        ]]))),

        ("<=1.2.3", ParsedSpec::Range(VersionConstraint(vec![vec![
            VersionComparator::Lte(semver(1, 2, 3, vec![], vec![]) ),
        ]]))),

        ("<1.2.3", ParsedSpec::Range(VersionConstraint(vec![vec![
            VersionComparator::Lt(semver(1, 2, 3, vec![], vec![]) ),
        ]]))),

        ("<1.2.3 <1.4.5", ParsedSpec::Range(VersionConstraint(vec![vec![
            VersionComparator::Lt(semver(1, 2, 3, vec![], vec![])),
            VersionComparator::Lt(semver(1, 4, 5, vec![], vec![])),
        ]]))),

        ("<1.2.3 <1.4.5 <1.6.3", ParsedSpec::Range(VersionConstraint(vec![vec![
            VersionComparator::Lt(semver(1, 2, 3, vec![], vec![])),
            VersionComparator::Lt(semver(1, 4, 5, vec![], vec![])),
            VersionComparator::Lt(semver(1, 6, 3, vec![], vec![])),
        ]]))),

        ("1.2.3 - 1.6.3", ParsedSpec::Range(VersionConstraint(vec![vec![
            VersionComparator::Gte(semver(1, 2, 3, vec![], vec![])),
            VersionComparator::Lte(semver(1, 6, 3, vec![], vec![])),
        ]]))),

        ("1.2.x", ParsedSpec::Range(VersionConstraint(vec![vec![
            VersionComparator::Gte(semver(1, 2, 0, vec![], vec![])),
            VersionComparator::Lt(semver(1, 3, 0, vec![PrereleaseTag::Int(0)], vec![])),
        ]]))),

        ("1.2.*", ParsedSpec::Range(VersionConstraint(vec![vec![
            VersionComparator::Gte(semver(1, 2, 0, vec![], vec![])),
            VersionComparator::Lt(semver(1, 3, 0, vec![PrereleaseTag::Int(0)], vec![])),
        ]]))),

        // Compare to 1.x.y in Tag section
        ("1.x", ParsedSpec::Range(VersionConstraint(vec![vec![
            VersionComparator::Gte(semver(1, 0, 0, vec![], vec![])),
            VersionComparator::Lt(semver(2, 0, 0, vec![PrereleaseTag::Int(0)], vec![])),
        ]]))),

        ("1.*", ParsedSpec::Range(VersionConstraint(vec![vec![
            VersionComparator::Gte(semver(1, 0, 0, vec![], vec![])),
            VersionComparator::Lt(semver(2, 0, 0, vec![PrereleaseTag::Int(0)], vec![])),
        ]]))),

        ("1.*.*", ParsedSpec::Range(VersionConstraint(vec![vec![
            VersionComparator::Gte(semver(1, 0, 0, vec![], vec![])),
            VersionComparator::Lt(semver(2, 0, 0, vec![PrereleaseTag::Int(0)], vec![])),
        ]]))),

        ("1.x.x", ParsedSpec::Range(VersionConstraint(vec![vec![
            VersionComparator::Gte(semver(1, 0, 0, vec![], vec![])),
            VersionComparator::Lt(semver(2, 0, 0, vec![PrereleaseTag::Int(0)], vec![])),
        ]]))),

        ("*", ParsedSpec::Range(VersionConstraint(vec![vec![
            VersionComparator::Any,
        ]]))),

        ("x", ParsedSpec::Range(VersionConstraint(vec![vec![
            VersionComparator::Any,
        ]]))),

        ("~1.2", ParsedSpec::Range(VersionConstraint(vec![vec![
            VersionComparator::Gte(semver(1, 2, 0, vec![], vec![])),
            VersionComparator::Lt(semver(1, 3, 0, vec![PrereleaseTag::Int(0)], vec![])),
        ]]))),

        ("~1.2.x", ParsedSpec::Range(VersionConstraint(vec![vec![
            VersionComparator::Gte(semver(1, 2, 0, vec![], vec![])),
            VersionComparator::Lt(semver(1, 3, 0, vec![PrereleaseTag::Int(0)], vec![])),
        ]]))),

        ("~1.2.*", ParsedSpec::Range(VersionConstraint(vec![vec![
            VersionComparator::Gte(semver(1, 2, 0, vec![], vec![])),
            VersionComparator::Lt(semver(1, 3, 0, vec![PrereleaseTag::Int(0)], vec![])),
        ]]))),

        ("~1.*.*", ParsedSpec::Range(VersionConstraint(vec![vec![
            VersionComparator::Gte(semver(1, 0, 0, vec![], vec![])),
            VersionComparator::Lt(semver(2, 0, 0, vec![PrereleaseTag::Int(0)], vec![])),
        ]]))),

        ("~1.x.x", ParsedSpec::Range(VersionConstraint(vec![vec![
            VersionComparator::Gte(semver(1, 0, 0, vec![], vec![])),
            VersionComparator::Lt(semver(2, 0, 0, vec![PrereleaseTag::Int(0)], vec![])),
        ]]))),

        ("~1", ParsedSpec::Range(VersionConstraint(vec![vec![
            VersionComparator::Gte(semver(1, 0, 0, vec![], vec![])),
            VersionComparator::Lt(semver(2, 0, 0, vec![PrereleaseTag::Int(0)], vec![])),
        ]]))),

        ("~*", ParsedSpec::Range(VersionConstraint(vec![vec![
            VersionComparator::Any
        ]]))),

        ("~x", ParsedSpec::Range(VersionConstraint(vec![vec![
            VersionComparator::Any
        ]]))),


        ("^1.2", ParsedSpec::Range(VersionConstraint(vec![vec![
            VersionComparator::Gte(semver(1, 2, 0, vec![], vec![])),
            VersionComparator::Lt(semver(2, 0, 0, vec![PrereleaseTag::Int(0)], vec![])),
        ]]))),

        ("^1.2.x", ParsedSpec::Range(VersionConstraint(vec![vec![
            VersionComparator::Gte(semver(1, 2, 0, vec![], vec![])),
            VersionComparator::Lt(semver(2, 0, 0, vec![PrereleaseTag::Int(0)], vec![])),
        ]]))),

        ("^1.2.*", ParsedSpec::Range(VersionConstraint(vec![vec![
            VersionComparator::Gte(semver(1, 2, 0, vec![], vec![])),
            VersionComparator::Lt(semver(2, 0, 0, vec![PrereleaseTag::Int(0)], vec![])),
        ]]))),

        ("^1.*.*", ParsedSpec::Range(VersionConstraint(vec![vec![
            VersionComparator::Gte(semver(1, 0, 0, vec![], vec![])),
            VersionComparator::Lt(semver(2, 0, 0, vec![PrereleaseTag::Int(0)], vec![])),
        ]]))),

        ("^1.x.x", ParsedSpec::Range(VersionConstraint(vec![vec![
            VersionComparator::Gte(semver(1, 0, 0, vec![], vec![])),
            VersionComparator::Lt(semver(2, 0, 0, vec![PrereleaseTag::Int(0)], vec![])),
        ]]))),

        ("^1", ParsedSpec::Range(VersionConstraint(vec![vec![
            VersionComparator::Gte(semver(1, 0, 0, vec![], vec![])),
            VersionComparator::Lt(semver(2, 0, 0, vec![PrereleaseTag::Int(0)], vec![])),
        ]]))),

        ("^*", ParsedSpec::Range(VersionConstraint(vec![vec![
            VersionComparator::Any
        ]]))),

        ("^x", ParsedSpec::Range(VersionConstraint(vec![vec![
            VersionComparator::Any
        ]]))),



        ("http://somewhere.com/blob.tgz", ParsedSpec::Remote("http://somewhere.com/blob.tgz".into())),
        ("https://mydomain.com/gitlab-org/gitlab", ParsedSpec::Remote("https://mydomain.com/gitlab-org/gitlab".into())), // compare to the Git section below

        ("some/dir/file.tgz", ParsedSpec::File("some/dir/file.tgz".into())),
        ("./some/file.tgz", ParsedSpec::File("./some/file.tgz".into())),

        ("./some/dir", ParsedSpec::Directory("./some/dir".into())),
        ("/some/dir", ParsedSpec::Directory("/some/dir".into())),
        ("some/other/dir", ParsedSpec::Directory("some/other/dir".into())),

        ("some/file.tgz", ParsedSpec::Git("github:some/file.tgz".into())),
        ("github:some/file.tgz", ParsedSpec::Git("github:some/file.tgz".into())),
        ("some/dir", ParsedSpec::Git("github:some/dir".into())),
        ("github:some/dir", ParsedSpec::Git("github:some/dir".into())),
        ("https://gitlab.com/gitlab-org/gitlab.git", ParsedSpec::Git("git+https://gitlab.com/gitlab-org/gitlab.git".into())),
        ("https://gitlab.com/gitlab-org/gitlab", ParsedSpec::Git("git+https://gitlab.com/gitlab-org/gitlab.git".into())),
        ("git@gitlab.com:gitlab-org/gitlab.git", ParsedSpec::Git("git+ssh://git@gitlab.com/gitlab-org/gitlab.git".into())),

        ("cats", ParsedSpec::Tag("cats".into())),
        ("", ParsedSpec::Tag("latest".into())),
        ("latest", ParsedSpec::Tag("latest".into())),
        ("1.x.y", ParsedSpec::Tag("1.x.y".into())),

        ("npm:bar@^1.2.3", ParsedSpec::Alias("bar".into(), None, AliasSubspec::Range(VersionConstraint(vec![vec![
            VersionComparator::Gte(semver_simple(1, 2, 3)),
            VersionComparator::Lt(semver(2, 0, 0, vec![PrereleaseTag::Int(0)], vec![]))
        ]])))),
        ("npm:bar@baz", ParsedSpec::Alias("bar".into(), None, AliasSubspec::Tag("baz".into()))),
        ("npm:@bar/baz@qux", ParsedSpec::Alias("@bar/baz".into(), None, AliasSubspec::Tag("qux".into()))),
        ("npm:@bar/baz@^1.2.3", ParsedSpec::Alias("@bar/baz".into(), None, AliasSubspec::Range(VersionConstraint(vec![vec![
            VersionComparator::Gte(semver_simple(1, 2, 3)),
            VersionComparator::Lt(semver(2, 0, 0, vec![PrereleaseTag::Int(0)], vec![]))
        ]])))),
    ];

    static ref INVALID_CASES: Vec<(&'static str, &'static str)> = vec![
        ("ht://stuff.cat", "EUNSUPPORTEDPROTOCOL"),
        ("^sp-reponse", "EINVALIDTAGNAME"),
    ];
}

#[test]
fn test_parse_spec_via_node_success_cases() {
    for (input, answer) in SUCCESS_CASES.iter() {
        println!("testing {}", input);
        assert_eq!(parse_spec_via_node(input).unwrap(), *answer)
    }
}

#[test]
fn test_parse_spec_via_node_invalid_cases() {
    for (input, err_contains) in INVALID_CASES.iter() {
        println!("testing {}", input);
        let spec = parse_spec_via_node(input).unwrap();
        match spec {
            ParsedSpec::Invalid(err) => assert!(err.contains(err_contains)),
            _ => panic!(),
        }
    }
}

fn semver_simple(major: i64, minor: i64, bug: i64) -> Semver {
    Semver {
        major,
        minor,
        bug,
        prerelease: vec![],
        build: vec![],
    }
}

fn semver(
    major: i64,
    minor: i64,
    bug: i64,
    prerelease: Vec<PrereleaseTag>,
    build: Vec<String>,
) -> Semver {
    Semver {
        major,
        minor,
        bug,
        prerelease,
        build,
    }
}

use semver_spec_serialization::parser::SpecParser;

// TODO: delete when done. this is a repl for debugging the spec parser
pub fn main() {
    // read string from stdin
    loop {
        let mut input = String::new();
        std::io::stdin().read_line(&mut input).unwrap();

        // parse the string using the semver parser
        dbg!(SpecParser::parse_str(input.trim_end_matches('\n')));
    }
}

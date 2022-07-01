use pest::{iterators::Pair, Parser};
use pest_derive::Parser;
use postgres_db::custom_types::ParsedSpec;

use crate::ParseSpecError;

#[derive(Parser)]
#[grammar = "grammar.pest"]
pub struct SpecParser;

impl SpecParser {
    pub fn parse_str(input: &str) -> Result<ParsedSpec, ParseSpecError> {
        let mut parser = SpecParser::parse(Rule::spec, input)?;
        let spec = parser.next().unwrap();

        println!("parsing main spec: {:?}", spec);

        Self::parse_inner(spec.into_inner().next().unwrap())
    }

    fn parse_inner(rule: Pair<Rule>) -> Result<ParsedSpec, ParseSpecError> {
        println!("parsing rule: {:?}", rule);
        match rule.as_rule() {
            Rule::filepath => {
                let mut path = String::new();
                for pair in rule.into_inner() {
                    path.push_str(pair.as_str());
                }
                Ok(ParsedSpec::File(path))
            }
            Rule::dirpath => {
                let mut path = String::new();
                for pair in rule.into_inner() {
                    path.push_str(pair.as_str());
                }
                Ok(ParsedSpec::Directory(path))
            }
            _ => Err(ParseSpecError::Other("todo".into())),
        }
    }
}

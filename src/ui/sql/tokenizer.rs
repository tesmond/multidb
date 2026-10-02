//! Port of the `@codemirror/lang-sql` tokenizer and dialect word lists. Used
//! for syntax highlighting, keyword completion and parser-style diagnostics.

use std::collections::HashMap;
use std::sync::LazyLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Tok {
    Whitespace,
    LineComment,
    BlockComment,
    String,
    Number,
    Bool,
    Null,
    ParenL,
    ParenR,
    BraceL,
    BraceR,
    BracketL,
    BracketR,
    Semi,
    Dot,
    Operator,
    Punctuation,
    SpecialVar,
    Identifier,
    QuotedIdentifier,
    Keyword,
    Type,
    Bits,
    Bytes,
    Builtin,
    /// A character the dialect cannot tokenize (a Lezer error node).
    Invalid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Token {
    pub kind: Tok,
    pub from: usize,
    pub to: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Dialect {
    Standard,
    PostgreSql,
    MySql,
    Sqlite,
}

impl Dialect {
    pub fn for_driver(driver: &str) -> Self {
        match driver {
            "mysql" => Dialect::MySql,
            "sqlite" => Dialect::Sqlite,
            _ => Dialect::PostgreSql,
        }
    }

    pub fn spec(self) -> &'static DialectSpec {
        match self {
            Dialect::Standard => &STANDARD,
            Dialect::PostgreSql => &POSTGRES,
            Dialect::MySql => &MYSQL,
            Dialect::Sqlite => &SQLITE,
        }
    }
}

pub struct DialectSpec {
    pub backslash_escapes: bool,
    pub hash_comments: bool,
    pub space_after_dashes: bool,
    pub slash_comments: bool,
    pub double_quoted_strings: bool,
    pub double_dollar_quoted_strings: bool,
    pub unquoted_bit_literals: bool,
    pub treat_bits_as_bytes: bool,
    pub char_set_casts: bool,
    pub plsql_quoting_mechanism: bool,
    pub operator_chars: &'static str,
    pub special_var: &'static str,
    pub identifier_quotes: &'static str,
    /// Lower-case word → token kind, in definition order (for completion).
    pub words: Vec<(String, Tok)>,
    pub word_map: HashMap<String, Tok>,
}

const SQL_TYPES: &str = "array binary bit boolean char character clob date decimal double float int integer interval large national nchar nclob numeric object precision real smallint time timestamp varchar varying ";
const SQL_KEYWORDS: &str = "absolute action add after all allocate alter and any are as asc assertion at authorization before begin between both breadth by call cascade cascaded case cast catalog check close collate collation column commit condition connect connection constraint constraints constructor continue corresponding count create cross cube current current_date current_default_transform_group current_transform_group_for_type current_path current_role current_time current_timestamp current_user cursor cycle data day deallocate declare default deferrable deferred delete depth deref desc describe descriptor deterministic diagnostics disconnect distinct do domain drop dynamic each else elseif end end-exec equals escape except exception exec execute exists exit external fetch first for foreign found from free full function general get global go goto grant group grouping handle having hold hour identity if immediate in indicator initially inner inout input insert intersect into is isolation join key language last lateral leading leave left level like limit local localtime localtimestamp locator loop map match method minute modifies module month names natural nesting new next no none not of old on only open option or order ordinality out outer output overlaps pad parameter partial path prepare preserve primary prior privileges procedure public read reads recursive redo ref references referencing relative release repeat resignal restrict result return returns revoke right role rollback rollup routine row rows savepoint schema scroll search second section select session session_user set sets signal similar size some space specific specifictype sql sqlexception sqlstate sqlwarning start state static system_user table temporary then timezone_hour timezone_minute to trailing transaction translation treat trigger under undo union unique unnest until update usage user using value values view when whenever where while with without work write year zone ";

const PG_KEYWORDS: &str = "abort abs absent access according ada admin aggregate alias also always analyse analyze array_agg array_max_cardinality asensitive assert assignment asymmetric atomic attach attribute attributes avg backward base64 begin_frame begin_partition bernoulli bit_length blocked bom cache called cardinality catalog_name ceil ceiling chain char_length character_length character_set_catalog character_set_name character_set_schema characteristics characters checkpoint class class_origin cluster coalesce cobol collation_catalog collation_name collation_schema collect column_name columns command_function command_function_code comment comments committed concurrently condition_number configuration conflict connection_name constant constraint_catalog constraint_name constraint_schema contains content control conversion convert copy corr cost covar_pop covar_samp csv cume_dist current_catalog current_row current_schema cursor_name database datalink datatype datetime_interval_code datetime_interval_precision db debug defaults defined definer degree delimiter delimiters dense_rank depends derived detach detail dictionary disable discard dispatch dlnewcopy dlpreviouscopy dlurlcomplete dlurlcompleteonly dlurlcompletewrite dlurlpath dlurlpathonly dlurlpathwrite dlurlscheme dlurlserver dlvalue document dump dynamic_function dynamic_function_code element elsif empty enable encoding encrypted end_frame end_partition endexec enforced enum errcode error event every exclude excluding exclusive exp explain expression extension extract family file filter final first_value flag floor following force foreach fortran forward frame_row freeze fs functions fusion generated granted greatest groups handler header hex hierarchy hint id ignore ilike immediately immutable implementation implicit import include including increment indent index indexes info inherit inherits inline insensitive instance instantiable instead integrity intersection invoker isnull key_member key_type label lag last_value lead leakproof least length library like_regex link listen ln load location lock locked log logged lower mapping matched materialized max max_cardinality maxvalue member merge message message_length message_octet_length message_text min minvalue mod mode more move multiset mumps name namespace nfc nfd nfkc nfkd nil normalize normalized nothing notice notify notnull nowait nth_value ntile nullable nullif nulls number occurrences_regex octet_length octets off offset oids operator options ordering others over overlay overriding owned owner parallel parameter_mode parameter_name parameter_ordinal_position parameter_specific_catalog parameter_specific_name parameter_specific_schema parser partition pascal passing passthrough password percent percent_rank percentile_cont percentile_disc perform period permission pg_context pg_datatype_name pg_exception_context pg_exception_detail pg_exception_hint placing plans pli policy portion position position_regex power precedes preceding prepared print_strict_params procedural procedures program publication query quote raise range rank reassign recheck recovery refresh regr_avgx regr_avgy regr_count regr_intercept regr_r2 regr_slope regr_sxx regr_sxy regr_syy reindex rename repeatable replace replica requiring reset respect restart restore result_oid returned_cardinality returned_length returned_octet_length returned_sqlstate returning reverse routine_catalog routine_name routine_schema routines row_count row_number rowtype rule scale schema_name schemas scope scope_catalog scope_name scope_schema security selective self sensitive sequence sequences serializable server server_name setof share show simple skip slice snapshot source specific_name sqlcode sqlerror sqrt stable stacked standalone statement statistics stddev_pop stddev_samp stdin stdout storage strict strip structure style subclass_origin submultiset subscription substring substring_regex succeeds sum symmetric sysid system system_time table_name tables tablesample tablespace temp template ties token top_level_count transaction_active transactions_committed transactions_rolled_back transform transforms translate translate_regex trigger_catalog trigger_name trigger_schema trim trim_array truncate trusted type types uescape unbounded uncommitted unencrypted unlink unlisten unlogged unnamed untyped upper uri use_column use_variable user_defined_type_catalog user_defined_type_code user_defined_type_name user_defined_type_schema vacuum valid validate validator value_of var_pop var_samp varbinary variable_conflict variadic verbose version versioning views volatile warning whitespace width_bucket window within wrapper xmlagg xmlattributes xmlbinary xmlcast xmlcomment xmlconcat xmldeclaration xmldocument xmlelement xmlexists xmlforest xmliterate xmlnamespaces xmlparse xmlpi xmlquery xmlroot xmlschema xmlserialize xmltable xmltext xmlvalidate yes";
const PG_TYPES: &str = "bigint int8 bigserial serial8 varbit bool box bytea cidr circle precision float8 inet int4 json jsonb line lseg macaddr macaddr8 money numeric pg_lsn point polygon float4 int2 smallserial serial2 serial serial4 text timetz timestamptz tsquery tsvector txid_snapshot uuid xml";

const MYSQL_KEYWORDS: &str = "accessible algorithm analyze asensitive authors auto_increment autocommit avg avg_row_length binlog btree cache catalog_name chain change changed checkpoint checksum class_origin client_statistics coalesce code collations columns comment committed completion concurrent consistent contains contributors convert database databases day_hour day_microsecond day_minute day_second delay_key_write delayed delimiter des_key_file dev_pop dev_samp deviance directory disable discard distinctrow div dual dumpfile enable enclosed ends engine engines enum errors escaped even event events every explain extended fast field fields flush force found_rows fulltext grants handler hash high_priority hosts hour_microsecond hour_minute hour_second ignore ignore_server_ids import index index_statistics infile innodb insensitive insert_method install invoker iterate keys kill linear lines list load lock logs low_priority master master_heartbeat_period master_ssl_verify_server_cert masters max max_rows maxvalue message_text middleint migrate min min_rows minute_microsecond minute_second mod mode modify mutex mysql_errno no_write_to_binlog offline offset one online optimize optionally outfile pack_keys parser partition partitions password phase plugin plugins prev processlist profile profiles purge query quick range read_write rebuild recover regexp relaylog remove rename reorganize repair repeatable replace require resume rlike row_format rtree schedule schema_name schemas second_microsecond security sensitive separator serializable server share show slave slow snapshot soname spatial sql_big_result sql_buffer_result sql_cache sql_calc_found_rows sql_no_cache sql_small_result ssl starting starts std stddev stddev_pop stddev_samp storage straight_join subclass_origin sum suspend table_name table_statistics tables tablespace terminated triggers truncate uncommitted uninstall unlock upgrade use use_frm user_resources user_statistics utc_date utc_time utc_timestamp variables views warnings xa xor year_month zerofill";
const MYSQL_TYPES: &str = "bool blob long longblob longtext medium mediumblob mediumint mediumtext tinyblob tinyint tinytext text bigint int1 int2 int3 int4 int8 float4 float8 varbinary varcharacter precision datetime unsigned signed";
const MYSQL_BUILTIN: &str = "charset clear edit ego help nopager notee nowarning pager print prompt quit rehash source status system tee";

const SQLITE_KEYWORDS: &str = "abort analyze attach autoincrement conflict database detach exclusive fail glob ignore index indexed instead isnull notnull offset plan pragma query raise regexp reindex rename replace temp vacuum virtual";
const SQLITE_TYPES: &str = "bool blob long longblob longtext medium mediumblob mediumint mediumtext tinyblob tinyint tinytext text bigint int2 int8 unsigned signed real";
const SQLITE_BUILTIN: &str = "auth backup bail changes clone databases dbinfo dump echo eqp explain fullschema headers help import imposter indexes iotrace lint load log mode nullvalue once print prompt quit restore save scanstats separator shell show stats system tables testcase timeout timer trace vfsinfo vfslist vfsname width";

/// Mirrors `keywords()` in lang-sql: later assignments overwrite earlier ones
/// but keep the original insertion position (JS object semantics).
fn build_words(keywords: &str, types: &str, builtin: &str) -> (Vec<(String, Tok)>, HashMap<String, Tok>) {
    let mut order: Vec<String> = Vec::new();
    let mut map: HashMap<String, Tok> = HashMap::new();
    let set = |w: &str, t: Tok, order: &mut Vec<String>, map: &mut HashMap<String, Tok>| {
        if !map.contains_key(w) {
            order.push(w.to_string());
        }
        map.insert(w.to_string(), t);
    };
    set("true", Tok::Bool, &mut order, &mut map);
    set("false", Tok::Bool, &mut order, &mut map);
    set("null", Tok::Null, &mut order, &mut map);
    set("unknown", Tok::Null, &mut order, &mut map);
    for w in keywords.split(' ').filter(|w| !w.is_empty()) {
        set(w, Tok::Keyword, &mut order, &mut map);
    }
    for w in types.split(' ').filter(|w| !w.is_empty()) {
        set(w, Tok::Type, &mut order, &mut map);
    }
    for w in builtin.split(' ').filter(|w| !w.is_empty()) {
        set(w, Tok::Builtin, &mut order, &mut map);
    }
    // JS objects enumerate integer-like keys first; none of these words are numeric.
    let words = order.iter().map(|w| (w.clone(), map[w])).collect();
    (words, map)
}

fn spec(
    f: impl FnOnce(&mut DialectSpec),
    keywords: Option<(String, String, &str)>,
) -> DialectSpec {
    let (words, word_map) = match &keywords {
        Some((k, t, b)) => build_words(k, t, b),
        None => build_words(SQL_KEYWORDS, SQL_TYPES, ""),
    };
    let mut s = DialectSpec {
        backslash_escapes: false,
        hash_comments: false,
        space_after_dashes: false,
        slash_comments: false,
        double_quoted_strings: false,
        double_dollar_quoted_strings: false,
        unquoted_bit_literals: false,
        treat_bits_as_bytes: false,
        char_set_casts: false,
        plsql_quoting_mechanism: false,
        operator_chars: "*+-%<>!=&|~^/",
        special_var: "?",
        identifier_quotes: "\"",
        words,
        word_map,
    };
    f(&mut s);
    s
}

static STANDARD: LazyLock<DialectSpec> = LazyLock::new(|| spec(|_| {}, None));
static POSTGRES: LazyLock<DialectSpec> = LazyLock::new(|| {
    spec(
        |s| {
            s.char_set_casts = true;
            s.double_dollar_quoted_strings = true;
            s.operator_chars = "+-*/<>=~!@#%^&|`?";
            s.special_var = "";
        },
        Some((format!("{SQL_KEYWORDS}{PG_KEYWORDS}"), format!("{SQL_TYPES}{PG_TYPES}"), "")),
    )
});
static MYSQL: LazyLock<DialectSpec> = LazyLock::new(|| {
    spec(
        |s| {
            s.operator_chars = "*+-%<>!=&|^";
            s.char_set_casts = true;
            s.double_quoted_strings = true;
            s.unquoted_bit_literals = true;
            s.hash_comments = true;
            s.space_after_dashes = true;
            s.special_var = "@?";
            s.identifier_quotes = "`";
        },
        Some((
            format!("{SQL_KEYWORDS}group_concat {MYSQL_KEYWORDS}"),
            format!("{SQL_TYPES}{MYSQL_TYPES}"),
            MYSQL_BUILTIN,
        )),
    )
});
static SQLITE: LazyLock<DialectSpec> = LazyLock::new(|| {
    spec(
        |s| {
            s.operator_chars = "*+-%<>!=&|/~";
            s.identifier_quotes = "`\"";
            s.special_var = "@:?$";
        },
        Some((
            format!("{SQL_KEYWORDS}{SQLITE_KEYWORDS}"),
            format!("{SQL_TYPES}{SQLITE_TYPES}"),
            SQLITE_BUILTIN,
        )),
    )
});

fn is_alpha(c: u32) -> bool {
    (65..=90).contains(&c) || (97..=122).contains(&c) || (48..=57).contains(&c)
}
fn is_hex(c: u32) -> bool {
    (48..=57).contains(&c) || (97..=102).contains(&c) || (65..=70).contains(&c)
}
fn in_str(c: Option<u32>, s: &str) -> bool {
    match c {
        Some(c) => char::from_u32(c).is_some_and(|ch| s.contains(ch)),
        None => false,
    }
}

const SPACE: &str = " \t\r\n";

struct Input<'a> {
    chars: &'a [(usize, char)],
    pos: usize,
    len_bytes: usize,
}

impl Input<'_> {
    fn next(&self) -> Option<u32> {
        self.chars.get(self.pos).map(|(_, c)| *c as u32)
    }
    fn peek(&self, offset: isize) -> Option<u32> {
        let idx = self.pos as isize + offset;
        if idx < 0 {
            return None;
        }
        self.chars.get(idx as usize).map(|(_, c)| *c as u32)
    }
    fn advance(&mut self) {
        if self.pos < self.chars.len() {
            self.pos += 1;
        }
    }
    fn advance_n(&mut self, n: usize) {
        for _ in 0..n {
            self.advance();
        }
    }
    fn byte_pos(&self) -> usize {
        self.chars.get(self.pos).map(|(b, _)| *b).unwrap_or(self.len_bytes)
    }
}

fn read_literal(input: &mut Input, end_quote: u32, backslash: bool) {
    let mut escaped = false;
    loop {
        let Some(n) = input.next() else { return };
        if n == end_quote && !escaped {
            input.advance();
            return;
        }
        escaped = backslash && !escaped && n == 92;
        input.advance();
    }
}

fn read_double_dollar(input: &mut Input, tag: &str) {
    'scan: loop {
        let Some(n) = input.next() else { return };
        if n == 36 {
            input.advance();
            for ch in tag.chars() {
                if input.next() != Some(ch as u32) {
                    continue 'scan;
                }
                input.advance();
            }
            if input.next() == Some(36) {
                input.advance();
                return;
            }
        } else {
            input.advance();
        }
    }
}

fn read_plsql_quoted(input: &mut Input, open: u32) {
    let open_c = char::from_u32(open).unwrap_or('\'');
    let close = match "[{<(".find(open_c) {
        Some(i) => "]}>)".chars().nth(i).unwrap() as u32,
        None => open,
    };
    loop {
        let Some(n) = input.next() else { return };
        if n == close && input.peek(1) == Some(39) {
            input.advance_n(2);
            return;
        }
        input.advance();
    }
}

fn read_word(input: &mut Input, mut result: Option<String>) -> Option<String> {
    loop {
        match input.next() {
            Some(c) if c == 95 || is_alpha(c) => {
                if let Some(r) = result.as_mut() {
                    r.push(char::from_u32(c).unwrap());
                }
                input.advance();
            }
            _ => break,
        }
    }
    result
}

fn read_word_or_quoted(input: &mut Input) {
    if matches!(input.next(), Some(39) | Some(34) | Some(96)) {
        let q = input.next().unwrap();
        input.advance();
        read_literal(input, q, false);
    } else {
        read_word(input, None);
    }
}

fn read_bits(input: &mut Input, end_quote: Option<u32>) {
    while matches!(input.next(), Some(48) | Some(49)) {
        input.advance();
    }
    if let Some(q) = end_quote {
        if input.next() == Some(q) {
            input.advance();
        }
    }
}

fn read_number(input: &mut Input, mut saw_dot: bool) {
    loop {
        match input.next() {
            Some(46) => {
                if saw_dot {
                    break;
                }
                saw_dot = true;
            }
            Some(c) if (48..=57).contains(&c) => {}
            _ => break,
        }
        input.advance();
    }
    if matches!(input.next(), Some(69) | Some(101)) {
        input.advance();
        if matches!(input.next(), Some(43) | Some(45)) {
            input.advance();
        }
        while matches!(input.next(), Some(c) if (48..=57).contains(&c)) {
            input.advance();
        }
    }
}

fn eol(input: &mut Input) {
    while !matches!(input.next(), None | Some(10)) {
        input.advance();
    }
}

/// Tokenize `text` with the given dialect. Offsets are byte offsets.
pub fn tokenize(text: &str, dialect: Dialect) -> Vec<Token> {
    let d = dialect.spec();
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let mut input = Input { chars: &chars, pos: 0, len_bytes: text.len() };
    let mut out = Vec::new();
    while input.pos < chars.len() {
        let start_pos = input.pos;
        let from = input.byte_pos();
        let next = input.next().unwrap();
        input.advance();
        let kind: Option<Tok> = if in_str(Some(next), SPACE) {
            while in_str(input.next(), SPACE) {
                input.advance();
            }
            Some(Tok::Whitespace)
        } else if next == 36 && d.double_dollar_quoted_strings {
            let tag = read_word(&mut input, Some(String::new())).unwrap_or_default();
            if input.next() == Some(36) {
                input.advance();
                read_double_dollar(&mut input, &tag);
                Some(Tok::String)
            } else {
                None
            }
        } else if next == 39 || (next == 34 && d.double_quoted_strings) {
            read_literal(&mut input, next, d.backslash_escapes);
            Some(Tok::String)
        } else if (next == 35 && d.hash_comments) || (next == 47 && input.next() == Some(47) && d.slash_comments) {
            eol(&mut input);
            Some(Tok::LineComment)
        } else if next == 45 && input.next() == Some(45) && (!d.space_after_dashes || input.peek(1) == Some(32)) {
            eol(&mut input);
            Some(Tok::LineComment)
        } else if next == 47 && input.next() == Some(42) {
            input.advance();
            let mut depth = 1;
            loop {
                let Some(cur) = input.next() else { break };
                input.advance();
                if cur == 42 && input.next() == Some(47) {
                    depth -= 1;
                    input.advance();
                    if depth == 0 {
                        break;
                    }
                } else if cur == 47 && input.next() == Some(42) {
                    depth += 1;
                    input.advance();
                }
            }
            Some(Tok::BlockComment)
        } else if (next == 101 || next == 69) && input.next() == Some(39) {
            input.advance();
            read_literal(&mut input, 39, true);
            Some(Tok::String)
        } else if (next == 110 || next == 78) && input.next() == Some(39) && d.char_set_casts {
            input.advance();
            read_literal(&mut input, 39, d.backslash_escapes);
            Some(Tok::String)
        } else if next == 95 && d.char_set_casts {
            let mut result = None;
            let mut i = 0;
            loop {
                if input.next() == Some(39) && i > 1 {
                    input.advance();
                    read_literal(&mut input, 39, d.backslash_escapes);
                    result = Some(Tok::String);
                    break;
                }
                if !input.next().is_some_and(is_alpha) {
                    break;
                }
                input.advance();
                i += 1;
            }
            result
        } else if d.plsql_quoting_mechanism
            && (next == 113 || next == 81)
            && input.next() == Some(39)
            && input.peek(1).is_some()
            && !in_str(input.peek(1), SPACE)
        {
            let open = input.peek(1).unwrap();
            input.advance_n(2);
            read_plsql_quoted(&mut input, open);
            Some(Tok::String)
        } else if in_str(Some(next), d.identifier_quotes) {
            let end = if next == 91 { 93 } else { next };
            read_literal(&mut input, end, false);
            Some(Tok::QuotedIdentifier)
        } else if next == 40 {
            Some(Tok::ParenL)
        } else if next == 41 {
            Some(Tok::ParenR)
        } else if next == 123 {
            Some(Tok::BraceL)
        } else if next == 125 {
            Some(Tok::BraceR)
        } else if next == 91 {
            Some(Tok::BracketL)
        } else if next == 93 {
            Some(Tok::BracketR)
        } else if next == 59 {
            Some(Tok::Semi)
        } else if d.unquoted_bit_literals && next == 48 && input.next() == Some(98) {
            input.advance();
            read_bits(&mut input, None);
            Some(Tok::Bits)
        } else if (next == 98 || next == 66) && matches!(input.next(), Some(39) | Some(34)) {
            let q = input.next().unwrap();
            input.advance();
            if d.treat_bits_as_bytes {
                read_literal(&mut input, q, d.backslash_escapes);
                Some(Tok::Bytes)
            } else {
                read_bits(&mut input, Some(q));
                Some(Tok::Bits)
            }
        } else if (next == 48 && matches!(input.next(), Some(120) | Some(88)))
            || ((next == 120 || next == 88) && input.next() == Some(39))
        {
            let quoted = input.next() == Some(39);
            input.advance();
            while input.next().is_some_and(is_hex) {
                input.advance();
            }
            if quoted && input.next() == Some(39) {
                input.advance();
            }
            Some(Tok::Number)
        } else if next == 46 && matches!(input.next(), Some(c) if (48..=57).contains(&c)) {
            read_number(&mut input, true);
            Some(Tok::Number)
        } else if next == 46 {
            Some(Tok::Dot)
        } else if (48..=57).contains(&next) {
            read_number(&mut input, false);
            Some(Tok::Number)
        } else if in_str(Some(next), d.operator_chars) {
            while in_str(input.next(), d.operator_chars) {
                input.advance();
            }
            Some(Tok::Operator)
        } else if in_str(Some(next), d.special_var) {
            if input.next() == Some(next) {
                input.advance();
            }
            read_word_or_quoted(&mut input);
            Some(Tok::SpecialVar)
        } else if next == 58 || next == 44 {
            Some(Tok::Punctuation)
        } else if is_alpha(next) {
            let word = read_word(&mut input, Some(char::from_u32(next).unwrap().to_string())).unwrap_or_default();
            let after_dot = input.next() == Some(46);
            let before_dot = input.peek(-(word.chars().count() as isize) - 1) == Some(46);
            if after_dot || before_dot {
                Some(Tok::Identifier)
            } else {
                Some(*d.word_map.get(&word.to_lowercase()).unwrap_or(&Tok::Identifier))
            }
        } else {
            None
        };
        match kind {
            Some(kind) => {
                out.push(Token { kind, from, to: input.byte_pos() });
            }
            None => {
                // Unrecognised input: Lezer skips one character as an error.
                input.pos = start_pos + 1;
                out.push(Token { kind: Tok::Invalid, from, to: input.byte_pos() });
            }
        }
    }
    out
}

/// Tokens that are not whitespace or comments.
pub fn significant(tokens: &[Token]) -> impl Iterator<Item = &Token> {
    tokens
        .iter()
        .filter(|t| !matches!(t.kind, Tok::Whitespace | Tok::LineComment | Tok::BlockComment))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(text: &str, d: Dialect) -> Vec<(Tok, &str)> {
        tokenize(text, d)
            .into_iter()
            .filter(|t| t.kind != Tok::Whitespace)
            .map(|t| (t.kind, &text[t.from..t.to]))
            .collect()
    }

    #[test]
    fn tokenizes_select() {
        let toks = kinds("SELECT a.id, 'x' FROM \"t\" WHERE n >= 1.5 -- c", Dialect::PostgreSql);
        assert_eq!(
            toks,
            vec![
                (Tok::Keyword, "SELECT"),
                (Tok::Identifier, "a"),
                (Tok::Dot, "."),
                (Tok::Identifier, "id"),
                (Tok::Punctuation, ","),
                (Tok::String, "'x'"),
                (Tok::Keyword, "FROM"),
                (Tok::QuotedIdentifier, "\"t\""),
                (Tok::Keyword, "WHERE"),
                (Tok::Identifier, "n"),
                (Tok::Operator, ">="),
                (Tok::Number, "1.5"),
                (Tok::LineComment, "-- c"),
            ]
        );
    }

    #[test]
    fn mysql_specifics() {
        let toks = kinds("select `a`, \"s\" # hi", Dialect::MySql);
        assert_eq!(toks[1], (Tok::QuotedIdentifier, "`a`"));
        assert_eq!(toks[3], (Tok::String, "\"s\""));
        assert_eq!(toks[4].0, Tok::LineComment);
        assert_eq!(kinds("a / b", Dialect::MySql)[1].0, Tok::Invalid);
        assert_eq!(kinds("text int", Dialect::Sqlite)[0].0, Tok::Type);
        assert_eq!(kinds("null true", Dialect::Sqlite)[0].0, Tok::Null);
    }
}

/// <reference types="tree-sitter-cli/dsl" />

module.exports = grammar({
  name: "oneway",

  extras: ($) => [/\s/],

  word: ($) => $.identifier,

  conflicts: ($) => [],

  rules: {
    source_file: ($) => repeat($._item),

    _item: ($) => choice($.use_decl, $.function_def, $.type_def),

    use_decl: ($) => seq("use", field("name", $.identifier)),

    // extern Rust("path")
    // Type.method = (params) -> Ret           (extern function declaration, no body)
    // Type.method = (params) -> Ret { body }  (normal method)
    // name       = (params) -> Ret { body }   (normal free function)
    // name       = (params) -> Ret            (trait-shaped — no body)
    function_def: ($) =>
      seq(
        optional(field("extern", $.extern_clause)),
        optional(seq(field("receiver", $.identifier), ".")),
        field("name", $.identifier),
        "=",
        optional(field("generics", $.generic_params)),
        field("params", $.param_list),
        "->",
        field("return_type", $._type),
        optional(field("body", $.block)),
      ),

    extern_clause: ($) =>
      seq(
        "extern",
        field("language", $.identifier),
        "(",
        field("path", $.string_literal),
        ")",
      ),

    // Name<Gen> = TypeExpr
    // extern Rust("...") Name = TypeExpr      (extern type alias)
    type_def: ($) =>
      seq(
        optional(field("extern", $.extern_clause)),
        field("name", $.identifier),
        optional(field("generics", $.generic_params)),
        "=",
        field("body", $._type),
      ),

    generic_params: ($) => seq("<", sep1($.generic_param, ","), ">"),

    generic_param: ($) =>
      seq(
        field("name", $.identifier),
        optional(seq(":", field("bound", $._type))),
      ),

    param_list: ($) => seq("(", optional(sep1($.param, ",")), ")"),

    param: ($) => seq(optional("mut"), field("type", $._type)),

    // Type expressions
    // Precedence (tightest first per NEW_DESIGN):
    //   T[N], ...T, T<...>, &, |
    _type: ($) => $._type_union,

    _type_union: ($) => choice($._type_product, $.union_type),

    union_type: ($) =>
      prec.left(seq($._type_product, repeat1(seq("|", $._type_product)))),

    _type_product: ($) => choice($._type_spread_or_lower, $.product_type),

    product_type: ($) =>
      prec.left(
        seq($._type_spread_or_lower, repeat1(seq("&", $._type_spread_or_lower))),
      ),

    _type_spread_or_lower: ($) =>
      choice($._type_repeat_or_lower, $.spread_type),

    spread_type: ($) => seq("...", $._type_repeat_or_lower),

    _type_repeat_or_lower: ($) => choice($._type_atom, $.repeat_type),

    repeat_type: ($) =>
      seq($._type_atom, "[", field("count", $.integer_literal), "]"),

    _type_atom: ($) => $.named_type,

    named_type: ($) =>
      seq(
        field("name", $.identifier),
        optional(seq("<", field("generics", sep1($._type, ",")), ">")),
      ),

    // Expressions
    block: ($) => seq("{", repeat($._expression), "}"),

    _expression: ($) =>
      choice(
        $.match_expression,
        $.while_expression,
        $.try_expression,
        $.method_call,
        $.constructor,
        $.identifier_expr,
        $.integer_literal,
        $.float_literal,
        $.hex_literal,
        $.string_literal,
      ),

    identifier_expr: ($) => $.identifier,

    constructor: ($) =>
      prec(
        1,
        seq(
          field("name", $.identifier),
          "(",
          optional(sep1($._expression, ",")),
          ")",
        ),
      ),

    method_call: ($) =>
      prec.left(
        2,
        seq(
          field("receiver", $._expression),
          ".",
          field("method", $.identifier),
          "(",
          optional(sep1($._expression, ",")),
          ")",
        ),
      ),

    try_expression: ($) =>
      prec.left(2, seq(field("inner", $._expression), "?")),

    match_expression: ($) =>
      seq(
        "match",
        field("scrutinee", $._expression),
        "{",
        optional(seq(sep1($.match_arm, ","), optional(","))),
        "}",
      ),

    match_arm: ($) =>
      seq(field("pattern", $._pattern), "=>", field("body", $._expression)),

    while_expression: ($) =>
      seq("while", field("cond", $._expression), field("body", $.block)),

    _pattern: ($) => choice($.wildcard_pattern, $.variant_pattern),

    wildcard_pattern: ($) => "_",

    variant_pattern: ($) =>
      seq(
        field("name", $.identifier),
        optional(seq("(", optional(sep1($._pattern, ",")), ")")),
      ),

    // Literals
    integer_literal: ($) => /[0-9]+/,
    float_literal: ($) => /[0-9]+\.[0-9]+/,
    hex_literal: ($) => /0x[0-9a-fA-F]+/,
    string_literal: ($) => /"([^"\\\n]|\\.)*"/,

    // Identifier — both camelCase and PascalCase share this lexeme.
    // Highlight queries use #match? to distinguish types from values.
    identifier: ($) => /[a-zA-Z_][a-zA-Z0-9_]*/,
  },
});

function sep1(rule, separator) {
  return seq(rule, repeat(seq(separator, rule)));
}

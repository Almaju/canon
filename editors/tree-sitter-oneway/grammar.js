/// <reference types="tree-sitter-cli/dsl" />

module.exports = grammar({
  name: "oneway",

  extras: ($) => [/\s/],

  word: ($) => $.identifier,

  conflicts: ($) => [
    [$._expression, $.struct_literal],
    [$._expression, $.enum_pattern],
    [$.function_type, $.union_type],
    [$.block, $.match_expression],
  ],

  rules: {
    source_file: ($) => repeat($._item),

    _item: ($) =>
      choice(
        $.use_declaration,
        $.newtype_declaration,
        $.struct_declaration,
        $.enum_declaration,
        $.contract_declaration,
        $.function_declaration,
      ),

    // Use
    use_declaration: ($) => seq("use", field("path", $.module_path)),

    module_path: ($) => seq($.identifier, repeat(seq(".", $.identifier))),

    // Newtype
    newtype_declaration: ($) =>
      seq(
        optional("pub"),
        "type",
        field("name", $.type_identifier),
        "=",
        field("type", $._type),
      ),

    // Struct
    struct_declaration: ($) =>
      seq(
        optional("pub"),
        "struct",
        field("name", $.type_identifier),
        "{",
        optional(
          seq(optional($.delegates_clause), commaSep($._type), optional(",")),
        ),
        "}",
      ),

    delegates_clause: ($) => repeat1(seq("delegates", $._type, optional(","))),

    // Enum
    enum_declaration: ($) =>
      seq(
        optional("pub"),
        "enum",
        field("name", $.type_identifier),
        "{",
        optional(seq(commaSep($.variant), optional(","))),
        "}",
      ),

    variant: ($) =>
      seq(field("name", $.type_identifier), optional(seq("(", $._type, ")"))),

    // Contract
    contract_declaration: ($) =>
      seq(
        optional("pub"),
        "contract",
        field("name", $.type_identifier),
        "{",
        optional(seq(commaSep($.contract_function), optional(","))),
        "}",
      ),

    contract_function: ($) =>
      seq(
        "fn",
        field("name", $.identifier),
        "(",
        optional(commaSep($._type)),
        ")",
        optional(seq("->", $._type)),
      ),

    // Function
    function_declaration: ($) =>
      seq(
        optional("pub"),
        "fn",
        field("name", $.identifier),
        "(",
        optional(commaSep($._type)),
        ")",
        optional(seq("->", field("return_type", $._type))),
        field("body", $.block),
      ),

    // Types
    _type: ($) =>
      choice(
        $.type_identifier,
        $.generic_type,
        $.function_type,
        $.union_type,
        "Self",
      ),

    generic_type: ($) => seq($.type_identifier, "<", commaSep1($._type), ">"),

    function_type: ($) =>
      prec(1, seq("fn", "(", optional(commaSep($._type)), ")", "->", $._type)),

    union_type: ($) => prec.left(seq($._type, "|", $._type)),

    // Expressions
    block: ($) => seq("{", repeat($._expression), "}"),

    _expression: ($) =>
      choice(
        $.integer_literal,
        $.float_literal,
        $.string_literal,
        $.boolean_literal,
        $.identifier,
        $.type_identifier,
        $.binary_expression,
        $.unary_expression,
        $.dot_expression,
        $.call_expression,
        $.struct_literal,
        $.match_expression,
        $.binding,
        $.try_expression,
        $.parenthesized_expression,
        $.block,
      ),

    integer_literal: ($) => /[0-9]+/,
    float_literal: ($) => /[0-9]+\.[0-9]+/,

    string_literal: ($) =>
      seq(
        '"',
        repeat(choice($.interpolation, $.escape_sequence, /[^"\\{]+/)),
        '"',
      ),

    interpolation: ($) => seq("{", $.identifier, "}"),
    escape_sequence: ($) => /\\./,

    boolean_literal: ($) => choice("true", "false"),

    binary_expression: ($) =>
      choice(
        ...[
          ["+", 6],
          ["-", 6],
          ["*", 7],
          ["/", 7],
          ["%", 7],
          ["==", 4],
          ["!=", 4],
          ["<", 5],
          [">", 5],
          ["<=", 5],
          [">=", 5],
          ["&&", 3],
          ["||", 2],
        ].map(([op, prec_val]) =>
          prec.left(
            prec_val,
            seq(
              field("left", $._expression),
              field("operator", op),
              field("right", $._expression),
            ),
          ),
        ),
      ),

    unary_expression: ($) =>
      prec(8, choice(seq("!", $._expression), seq("-", $._expression))),

    dot_expression: ($) =>
      prec.left(
        9,
        seq(field("object", $._expression), ".", field("field", $.identifier)),
      ),

    call_expression: ($) =>
      prec.left(
        9,
        seq(
          field("function", $._expression),
          "(",
          optional(field("argument", $._expression)),
          ")",
        ),
      ),

    struct_literal: ($) =>
      prec(
        10,
        seq(
          field("type", $.type_identifier),
          "{",
          optional(seq(commaSep($._expression), optional(","))),
          "}",
        ),
      ),

    match_expression: ($) =>
      seq(
        "match",
        optional(field("subject", $._expression)),
        "{",
        optional(seq(commaSep($.match_arm), optional(","))),
        "}",
      ),

    match_arm: ($) =>
      seq(field("pattern", $._pattern), "=>", field("body", $._expression)),

    _pattern: ($) =>
      choice(
        "_",
        $.integer_literal,
        $.float_literal,
        $.string_literal,
        $.boolean_literal,
        $.identifier,
        $.enum_pattern,
      ),

    enum_pattern: ($) =>
      seq(
        $.type_identifier,
        ".",
        $.type_identifier,
        optional(seq("(", $._pattern, ")")),
      ),

    binding: ($) =>
      prec.right(
        1,
        seq(field("name", $.identifier), "=", field("value", $._expression)),
      ),

    try_expression: ($) => prec.left(9, seq($._expression, "?")),

    parenthesized_expression: ($) => seq("(", $._expression, ")"),

    // Identifiers
    identifier: ($) => /[a-z_][a-zA-Z0-9_]*/,
    type_identifier: ($) => /[A-Z][a-zA-Z0-9_]*/,
  },
});

function commaSep(rule) {
  return optional(commaSep1(rule));
}

function commaSep1(rule) {
  return seq(rule, repeat(seq(",", rule)));
}

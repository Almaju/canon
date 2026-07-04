/// <reference types="tree-sitter-cli/dsl" />

module.exports = grammar({
  name: "canon",

  extras: ($) => [/\s/],

  word: ($) => $.identifier,

  conflicts: ($) => [],

  rules: {
    source_file: ($) => repeat($._item),

    _item: ($) =>
      choice(
        $.bindings_decl,
        $.package_decl,
        $.function_def,
        $.type_def,
        $.extern_type_decl,
      ),

    // bindings "wasi:random/random@0.3.0"   (generated binding files)
    bindings_decl: ($) => seq("bindings", field("urn", $.string_literal)),

    // package "acme:http@1.2.3"             (vendored files under deps/)
    package_decl: ($) =>
      seq("package", field("coordinate", $.string_literal)),

    // extern Rust("path")
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
        optional(seq(".", field("qualifier", $.identifier))),
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

    // Bare extern type declaration: extern Rust("...") TypeName
    // Now supports generic params: extern Rust("Foo") Bar<S>
    extern_type_decl: ($) =>
      seq(
        field("extern", $.extern_clause),
        field("name", $.identifier),
        optional(field("generics", $.generic_params)),
      ),

    generic_params: ($) => seq("<", sep1($.generic_param, ","), ">"),

    generic_param: ($) =>
      seq(
        field("name", $.identifier),
        optional(seq(":", field("bound", $._type))),
      ),

    param_list: ($) => seq("(", optional(sep1($.param, ",")), ")"),

    param: ($) => seq(optional("mut"), field("type", $._type)),

    // Type expressions — precedence (tightest first):
    //   T^N / T^*, T<...>, *, +
    _type: ($) => $._type_union,

    _type_union: ($) => choice($._type_product, $.union_type),

    union_type: ($) =>
      prec.left(seq($._type_product, repeat1(seq("+", $._type_product)))),

    _type_product: ($) => choice($._type_postfix, $.product_type),

    product_type: ($) =>
      prec.left(seq($._type_postfix, repeat1(seq("*", $._type_postfix)))),

    _type_postfix: ($) => choice($._type_atom, $.repeat_type, $.spread_type),

    repeat_type: ($) =>
      seq($._type_atom, "^", field("count", $.integer_literal)),

    spread_type: ($) => seq($._type_atom, "^", "*"),

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
        $.dispatch,
        $.try_expression,
        $.method_call,
        $.constructor,
        $.lambda,
        $.identifier_expr,
        $.integer_literal,
        $.float_literal,
        $.hex_literal,
        $.string_literal,
        $.json_object,
        $.json_array,
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
          optional(seq("::", "<", field("type_args", sep1($._type, ",")), ">")),
          "(",
          optional(sep1($._expression, ",")),
          ")",
        ),
      ),

    // Dispatch: value.( * (Type) -> RetType { body } * (Type) -> RetType { body } )
    // The `*` before each arm is enforced by the formatter; the grammar accepts
    // it as optional before the first arm for resilience.
    dispatch: ($) =>
      prec.left(
        2,
        seq(
          field("scrutinee", $._expression),
          ".",
          "(",
          repeat(seq(optional("*"), field("arms", $.dispatch_arm))),
          ")",
        ),
      ),

    // Each dispatch arm is a lambda: (VariantType) -> ReturnType { body }
    // The VariantType may carry generic type args: Ok<Int>, Err<String>, Some<T>
    dispatch_arm: ($) =>
      seq(
        "(",
        field("param_type", $.named_type),
        ")",
        "->",
        field("return_type", $._type),
        field("body", $.block),
      ),

    try_expression: ($) =>
      prec.left(2, seq(field("inner", $._expression), "?")),

    // Lambda: (Type) -> RetType { body }
    lambda: ($) =>
      seq(
        field("params", $.param_list),
        "->",
        field("return_type", $._type),
        field("body", $.block),
      ),

    // JSON literals
    json_object: ($) => seq("{", optional(sep1($.json_pair, ",")), "}"),

    json_pair: ($) =>
      seq(field("key", $.string_literal), ":", field("value", $.json_value)),

    json_array: ($) => seq("[", optional(sep1($.json_value, ",")), "]"),

    json_value: ($) =>
      choice(
        $.json_object,
        $.json_array,
        $.string_literal,
        seq("-", choice($.integer_literal, $.float_literal)),
        $.integer_literal,
        $.float_literal,
        $.identifier,
      ),

    // Literals
    integer_literal: ($) => /[0-9]+/,
    float_literal: ($) => /[0-9]+\.[0-9]+/,
    hex_literal: ($) => /0x[0-9a-fA-F]+/,
    // String literals support backslash escape sequences
    string_literal: ($) => /"([^"\\\n]|\\.)*"/,

    // Identifier — both camelCase and PascalCase share this lexeme.
    // Highlight queries use #match? to distinguish types vs values.
    identifier: ($) => /[a-zA-Z_][a-zA-Z0-9_]*/,
  },
});

function sep1(rule, separator) {
  return seq(rule, repeat(seq(separator, rule)));
}

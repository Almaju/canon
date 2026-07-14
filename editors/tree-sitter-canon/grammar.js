/// <reference types="tree-sitter-cli/dsl" />

// Tree-sitter grammar for Canon (types-only Canon).
//
// This is a pragmatic highlighting grammar, not a full reimplementation of
// the compiler's parser. It is deliberately permissive: it accepts a
// superset of Canon (the checker is the backstop), but it parses the real
// corpus — stdlib, tests, examples, binding files — without ERROR nodes.
//
// The shapes it knows:
//   Type def            Bool = False + True          Node = Key * Rest * Value
//   Anonymous ctor      Map * String => Contains { … }      Unit => Program { … }
//   Named decl / FFI    parallel = <T>(Future<T> * Future<T>) => Future<List<T>>
//                       Response = (Body * Headers * Status) => Response
//   Pipe                value -> Name(args) -> Other?
//   Dispatch            value -> ( * Variant => Type { … } * Other => Type { … } )
//   Lambda              (Int) => Int { Int -> Product(2) }
//   Literals            42  -5  1.5  "s"  `fmt {expr}`  {"k":v}  [1,2]  <div>{x}</div>

module.exports = grammar({
  name: "canon",

  extras: ($) => [/\s/],

  word: ($) => $.identifier,

  conflicts: ($) => [[$.generic_param, $.named_type]],

  rules: {
    source_file: ($) => repeat($._item),

    _item: ($) => choice($.type_def, $.named_def, $.constructor_def),

    // Name = TypeExpr        Name<T> = TypeExpr
    type_def: ($) =>
      seq(
        field("name", $.identifier),
        optional(field("generics", $.generic_params)),
        "=",
        field("body", $._type),
      ),

    // name = <T>(A * B) => Ret          (body-less FFI / callback alias)
    // Name = (A) => B { body }          (named declaration)
    named_def: ($) =>
      seq(
        field("name", $.identifier),
        "=",
        optional(field("generics", $.generic_params)),
        field("params", $.param_list),
        "=>",
        field("return_type", $._type),
        optional(field("body", $.block)),
      ),

    // (A * B) => C { body }
    // Request => Response { body }      (paren-free input)
    // Map * String => Contains { body } (paren-free product input)
    // Unit => Program { body }          (nullary)
    constructor_def: ($) =>
      seq(
        field("input", choice($.param_list, $.input_product)),
        "=>",
        field("return_type", $._type),
        field("body", $.block),
      ),

    input_product: ($) => prec.right(sep1($.named_type, "*")),

    generic_params: ($) => seq("<", sep1($.generic_param, ","), ">"),

    generic_param: ($) => $.identifier,

    // ─── Types ───────────────────────────────────────────────────────────

    _type: ($) => choice($.union_type, $._type_product),

    union_type: ($) =>
      prec.left(seq($._type_product, repeat1(seq("+", $._type_product)))),

    _type_product: ($) => choice($.product_type, $._type_postfix),

    product_type: ($) =>
      prec.left(seq($._type_postfix, repeat1(seq("*", $._type_postfix)))),

    _type_postfix: ($) => choice($.repeat_type, $._type_atom),

    // Byte^8    Bytes = Byte^*
    repeat_type: ($) =>
      seq($._type_atom, "^", field("count", choice($.integer_literal, "*"))),

    _type_atom: ($) => choice($.named_type, $.param_list),

    named_type: ($) =>
      seq(
        field("name", $.identifier),
        optional(field("type_args", $.type_args)),
      ),

    type_args: ($) => seq("<", sep1($._param_type, ","), ">"),

    // (A * B)    ()    (Some<T>)    (() => Option<Stream<T> * T>)
    param_list: ($) => seq("(", optional($._param_type), ")"),

    _param_type: ($) => choice($.function_type, $._type),

    // (A) => B    () => Stream<T>     (nested callback / FFI alias types)
    function_type: ($) =>
      prec.right(
        seq(
          field("params", $.param_list),
          "=>",
          field("return_type", $._type),
        ),
      ),

    // ─── Expressions ─────────────────────────────────────────────────────

    block: ($) => seq("{", repeat($._expression), "}"),

    _expression: ($) => choice($.pipe_expression, $._postfix_expression),

    // value -> Name(args) -> Other    value -> ( * A => T { … } … )
    pipe_expression: ($) =>
      prec.left(
        seq(
          field("left", $._expression),
          "->",
          field("right", choice($.dispatch, $._postfix_expression)),
        ),
      ),

    // The arm group a scrutinee pipes into.
    dispatch: ($) =>
      seq("(", repeat1(seq(optional("*"), field("arm", $.dispatch_arm))), ")"),

    // * Some<Value> => Contains { … }    * "GET" => Response { … }    * 34 => X { … }
    dispatch_arm: ($) =>
      seq(
        field(
          "pattern",
          choice($.named_type, $.string_literal, $.integer_literal),
        ),
        "=>",
        field("return_type", $._type),
        field("body", $.block),
      ),

    _postfix_expression: ($) =>
      choice($._atom, $.try_expression, $.field_access, $.method_call),

    try_expression: ($) => prec.left(seq($._postfix_expression, "?")),

    // user.Birthday    Node.Rest    tuple.1
    field_access: ($) =>
      prec.left(
        1,
        seq(
          field("receiver", $._postfix_expression),
          ".",
          field("field", choice($.identifier, $.integer_literal)),
        ),
      ),

    // req.method()    Headers().set("k" * "v")    (camelCase = FFI boundary)
    method_call: ($) =>
      prec.left(
        2,
        seq(
          field("receiver", $._postfix_expression),
          ".",
          field("method", $.identifier),
          "(",
          optional(field("arguments", $.arguments)),
          ")",
        ),
      ),

    _atom: ($) =>
      choice(
        $.call,
        $.identifier_expr,
        $.lambda,
        $.integer_literal,
        $.float_literal,
        $.string_literal,
        $.format_string,
        $.json_object,
        $.json_array,
        $.html_element,
      ),

    // Greeting("hi")    List(1 * 2 * 3)    Empty()
    call: ($) =>
      prec(
        1,
        seq(
          field("name", $.identifier),
          "(",
          optional(field("arguments", $.arguments)),
          ")",
        ),
      ),

    // Args are `*`-separated full expressions: Substring(From(2) * Line -> Length -> To)
    arguments: ($) => sep1($._expression, "*"),

    identifier_expr: ($) => $.identifier,

    // (Int) => Int { Int -> Product(2) }
    lambda: ($) =>
      seq(
        field("params", $.param_list),
        "=>",
        field("return_type", $._type),
        field("body", $.block),
      ),

    // ─── Format strings ──────────────────────────────────────────────────

    // `count is {Int}, doubled {Int -> Product(2)}` — {{ }} escape braces.
    format_string: ($) =>
      seq("`", repeat(choice($.format_text, $.interpolation)), "`"),

    format_text: ($) => token(prec(1, /([^`{}\\]|\\.|\{\{|\}\})+/)),

    interpolation: ($) => seq("{", $._expression, "}"),

    // ─── JSON literals ───────────────────────────────────────────────────

    json_object: ($) => seq("{", optional(sep1($.json_pair, ",")), "}"),

    json_pair: ($) =>
      seq(field("key", $.string_literal), ":", field("value", $._expression)),

    json_array: ($) => seq("[", optional(sep1($._expression, ",")), "]"),

    // ─── HTML literals ───────────────────────────────────────────────────

    // <div class="box">hello {expr}</div> — permissive: the close tag is not
    // checked against the open tag (the checker is the backstop).
    html_element: ($) =>
      choice(
        seq(
          $.html_open_tag,
          repeat($._html_content),
          $.html_close_tag,
        ),
        $.html_void_tag,
        $.html_self_closing_tag,
      ),

    // Void elements never take a close tag: <hr> <br> <img src="…">
    html_void_tag: ($) =>
      seq(
        "<",
        field(
          "name",
          alias($._html_void_tag_name, $.html_tag_name),
        ),
        repeat($.html_attribute),
        choice(">", "/>"),
      ),

    _html_void_tag_name: ($) =>
      token(
        prec(
          1,
          /area|base|br|col|embed|hr|img|input|link|meta|param|source|track|wbr/,
        ),
      ),

    html_open_tag: ($) =>
      seq(
        "<",
        field("name", $.html_tag_name),
        repeat($.html_attribute),
        ">",
      ),

    html_self_closing_tag: ($) =>
      seq(
        "<",
        field("name", $.html_tag_name),
        repeat($.html_attribute),
        "/>",
      ),

    html_close_tag: ($) => seq("</", field("name", $.html_tag_name), ">"),

    html_attribute: ($) =>
      seq(
        field("name", $.html_attr_name),
        optional(seq("=", field("value", $.string_literal))),
      ),

    html_tag_name: ($) => /[a-z][a-zA-Z0-9-]*/,

    html_attr_name: ($) => /[a-z][a-zA-Z0-9-]*/,

    _html_content: ($) =>
      choice($.html_text, $.interpolation, $.html_element),

    html_text: ($) => token(/([^<{}\s]|\{\{|\}\})([^<{}]|\{\{|\}\})*/),

    // ─── Literals ────────────────────────────────────────────────────────

    integer_literal: ($) => /-?[0-9]+/,
    float_literal: ($) => /-?[0-9]+\.[0-9]+/,
    // String literals support backslash escape sequences
    string_literal: ($) => /"([^"\\\n]|\\.)*"/,

    // Identifier — PascalCase (types, the only names) and camelCase (FFI)
    // share this lexeme. Highlight queries use #match? to distinguish.
    identifier: ($) => /[a-zA-Z_][a-zA-Z0-9_]*/,
  },
});

function sep1(rule, separator) {
  return seq(rule, repeat(seq(separator, rule)));
}

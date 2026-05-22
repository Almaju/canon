; ─── Keywords ─────────────────────────────────────────────────────────────────
"use" @keyword
"mut" @keyword
"extern" @keyword

; ─── Operators ────────────────────────────────────────────────────────────────
"=" @operator
"->" @operator
"=>" @operator
"+" @operator
"*" @operator
"?" @operator
"^" @operator
"::" @operator

; ─── Punctuation ──────────────────────────────────────────────────────────────
[
  "("
  ")"
  "{"
  "}"
  "<"
  ">"
] @punctuation.bracket

[
  "."
  ","
  ":"
] @punctuation.delimiter

; ─── Literals ─────────────────────────────────────────────────────────────────
(integer_literal) @number
(float_literal) @number
(hex_literal) @number
(string_literal) @string

; ─── Wildcard ─────────────────────────────────────────────────────────────────
(wildcard_pattern) @variable.special

; ─── Definitions ──────────────────────────────────────────────────────────────

; Function definition: receiver (PascalCase) + name (camelCase or PascalCase)
(function_def receiver: (identifier) @type)
(function_def name: (identifier) @function)

; Type definition name
(type_def name: (identifier) @type)

; Bare extern type declaration
(extern_type_decl name: (identifier) @type)

; Use declaration
(use_decl name: (identifier) @namespace)

; Extern clause
(extern_clause language: (identifier) @type.builtin)
(extern_clause qualifier: (identifier) @keyword)
(extern_clause path: (string_literal) @string.special)

; Generic params
(generic_param name: (identifier) @type.parameter)

; Parameters: the type of each param
(param type: (named_type name: (identifier) @type))

; ─── Type Expressions ─────────────────────────────────────────────────────────

; Names inside type expressions are types
(named_type name: (identifier) @type)

; ─── Expressions ──────────────────────────────────────────────────────────────

; Method calls
(method_call method: (identifier) @function.method)

; Constructor calls: PascalCase constructors are styled as @type because in Oneway
; the constructor name IS the type name (e.g. Greeting("hi") creates a Greeting).
; This visually separates them from method calls (@function.method) in all themes.
((constructor name: (identifier) @function.call)
  (#match? @function.call "^[a-z_]"))

((constructor name: (identifier) @type)
  (#match? @type "^[A-Z]"))

; Plain identifier in expression position — distinguish PascalCase vs camelCase
((identifier_expr (identifier) @type)
  (#match? @type "^[A-Z]"))

((identifier_expr (identifier) @variable)
  (#match? @variable "^[a-z_]"))

; Pattern variant names: PascalCase variants styled as @type, lowercase as @variable
((variant_pattern name: (identifier) @type)
  (#match? @type "^[A-Z]"))

((variant_pattern name: (identifier) @variable)
  (#match? @variable "^[a-z_]"))

; ─── Lambda ───────────────────────────────────────────────────────────────────

(lambda "->" @operator)

; ─── Special Names ────────────────────────────────────────────────────────────

; `Self` is a builtin
((identifier) @type.builtin
  (#eq? @type.builtin "Self"))

; Built-in types, capabilities, and well-known constructors
((identifier) @type.builtin
  (#any-of? @type.builtin
    "Bit" "Byte" "Bytes" "Off" "On"
    "Int" "Float" "Hex" "String"
    "Bool" "False" "True"
    "Ord" "Equal" "Greater" "Less"
    "Option" "Some" "None"
    "Result" "Ok" "Err"
    "Unit" "Never"
    "List" "Map"
    "Clock" "Filesystem" "Network" "Random"
    "Stderr" "Stdin" "Stdout"))

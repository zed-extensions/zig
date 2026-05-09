; Tag unit tests
((test_declaration
  (string
    (string_content) @name @ZIG_TEST_NAME)) @run
  (#set! tag zig-test))

((test_declaration
  (identifier) @name @ZIG_DOCTEST_NAME) @run
  (#set! tag zig-doc-test))

; Tag main
((function_declaration
  name: (identifier) @_name) @run
  (#match? @_name "main")
  (#set! tag zig-build-run)
  (#set! tag zig-run))

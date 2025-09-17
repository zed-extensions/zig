; Tag unit tests
(
    (test_declaration
        (string) @name @ZIG_TEST_NAME
    ) @run @_zig-test
    (#set! tag zig-test)
)

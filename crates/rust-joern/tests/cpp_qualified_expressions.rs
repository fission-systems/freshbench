//! C++ expression-name regressions. These check syntax, call operands and CFG
//! ordering; they are not snapshots generated from this implementation.
use rust_joern::{
    analyze,
    graph::{Analysis, FunctionGraph, Options},
};

fn parse(source: &str) -> Analysis {
    analyze(
        source,
        "qualified.cpp",
        &Options {
            language: Some("cpp".into()),
            strict: true,
            preprocessed: true,
            ..Options::default()
        },
    )
    .unwrap()
}
fn function(analysis: &Analysis) -> &FunctionGraph {
    analysis.functions.iter().find(|f| f.name == "f").unwrap()
}
fn call(function: &FunctionGraph, name: &str) -> u32 {
    let ids: Vec<_> = function
        .cpg
        .nodes
        .iter()
        .filter(|n| n.kind == "CALL" && n.name.as_deref() == Some(name))
        .map(|n| n.id)
        .collect();
    assert_eq!(ids.len(), 1, "expected exactly one {name}: {ids:?}");
    ids[0]
}
fn path(function: &FunctionGraph, from: u32, to: u32) -> bool {
    let mut pending = vec![from];
    let mut seen = std::collections::HashSet::new();
    while let Some(node) = pending.pop() {
        if node == to {
            return true;
        }
        if seen.insert(node) {
            pending.extend(
                function
                    .cpg
                    .edges
                    .iter()
                    .filter(|e| e.kind == "CFG" && e.source == node)
                    .map(|e| e.target),
            );
        }
    }
    false
}
fn argument_code(function: &FunctionGraph, call: u32, index: usize) -> &str {
    let edge = function
        .cpg
        .edges
        .iter()
        .find(|e| {
            e.kind == "ARGUMENT"
                && e.source == call
                && e.label.as_deref() == Some(&index.to_string())
        })
        .unwrap();
    &function.cpg.nodes[edge.target as usize].code
}

#[test]
fn declared_template_calls_remain_calls_in_all_expression_positions() {
    let context = "namespace demo { template<class T> struct Box { static int get(int); static void touch(int); }; } int sink(int);";
    for body in [
        "demo::Box<int>::touch(x);",
        "return demo::Box<int>::get(x);",
        "return sink(demo::Box<int>::get(x));",
    ] {
        let analysis = parse(&format!("{context} int f(int x) {{ {body} }}"));
        let f = function(&analysis);
        let name = if body.starts_with("demo") {
            "touch"
        } else {
            "get"
        };
        let id = call(f, name);
        assert_eq!(argument_code(f, id, 1), "x");
        assert!(
            f.cpg.nodes[id as usize]
                .code
                .starts_with("demo::Box<int>::")
        );
        if body.contains("sink") {
            assert!(path(f, id, call(f, "sink")));
        }
    }
}

#[test]
fn nested_template_ids_and_global_scope_keep_callee_and_operands() {
    let a = parse(
        "namespace demo { template<class T> struct Box { static int get(int,int); }; } int left(); int right(); int f() { return ::demo::Box<demo::Box<int>>::get(left(),right()); }",
    );
    let f = function(&a);
    let id = call(f, "get");
    assert_eq!(argument_code(f, id, 1), "left()");
    assert_eq!(argument_code(f, id, 2), "right()");
    assert!(path(f, call(f, "left"), id));
    assert!(path(f, call(f, "right"), id));
    assert_eq!(
        f.cpg.nodes[id as usize].code,
        "::demo::Box<demo::Box<int>>::get(left(),right())"
    );
}

#[test]
fn explicit_function_specializations_keep_each_written_body() {
    let analysis = parse(
        "namespace demo {
int tick(int);
template<int N> int convert(int value);
template<> int convert<4>(int value) { if (value) return tick(value); return 4; }
template<> int convert<8>(int value) { return tick(value + 8); }
}
int after(int value) { return value + 1; }",
    );
    let convert: Vec<_> = analysis
        .functions
        .iter()
        .filter(|f| f.name == "convert")
        .collect();
    assert_eq!(convert.len(), 2);
    for f in &convert {
        assert_eq!(f.fullname, "demo.convert:int(int)");
        assert_eq!(f.return_type, "int");
        call(f, "tick");
    }
    assert_eq!((convert[0].start_line, convert[0].end_line), (4, 4));
    assert_eq!((convert[1].start_line, convert[1].end_line), (5, 5));
    assert!(convert[0].cfg.nodes.len() > convert[1].cfg.nodes.len());
    assert!(
        analysis
            .functions
            .iter()
            .any(|f| f.name == "after" && f.start_line == 7)
    );
}

#[test]
fn explicit_specialization_with_nested_template_arguments_keeps_its_return() {
    let analysis = parse(
        "template<class T> struct Box {};
         template<class T> int f(int x);
         template<> int f<Box<int>>(int x) { return x + 1; }",
    );
    let f = function(&analysis);
    assert_eq!(f.fullname, "f:int(int)");
    assert_eq!((f.start_line, f.end_line), (3, 3));
    call(f, "<operator>.addition");
}

#[test]
fn variable_template_bound_keeps_the_loop_comparison_and_back_edge() {
    let analysis = parse(
        "namespace demo { template<class T, int N> constexpr int bound = N; }
         void tick(int);
         template<class T> int f() {
             for (int i = 0; i < demo::bound<T, 4>; ++i) tick(i);
             return 0;
         }",
    );
    let f = function(&analysis);
    let comparison = call(f, "<operator>.lessThan");
    let body = call(f, "tick");
    let increment = call(f, "<operator>.preIncrement");
    assert_eq!(argument_code(f, comparison, 1), "i");
    assert_eq!(argument_code(f, comparison, 2), "demo::bound<T, 4>");
    assert!(path(f, comparison, body));
    assert!(path(f, body, increment));
    assert!(path(f, increment, comparison));
    assert!(
        f.cpg
            .nodes
            .iter()
            .all(|node| node.name.as_deref() != Some("<operator>.greaterThan"))
    );
}

#[test]
fn immediate_lambda_call_remains_a_comparison_operand() {
    for (lambda, input) in [
        ("[n] { return n; }()", None),
        ("[n](int x) { return n + x; }(n)", Some("n")),
    ] {
        let analysis = parse(&format!("int f(int n) {{ return n > {lambda}; }}"));
        let f = function(&analysis);
        let comparison = call(f, "<operator>.greaterThan");
        let invoke = call(f, "<operator>()");
        assert_eq!(argument_code(f, comparison, 2), lambda);
        if let Some(input) = input {
            assert_eq!(argument_code(f, invoke, 1), input);
        }
        assert!(path(f, invoke, comparison));
        assert!(
            analysis
                .functions
                .iter()
                .any(|method| method.name.starts_with("<lambda>"))
        );
    }
}

#[test]
fn enclosing_function_remains_bindable_after_lambda_recovery() {
    let analysis = parse(
        "int source() { return [] { return 1; }(); }
         int f() { auto p = source; return p(); }",
    );
    assert!(
        analysis
            .functions
            .iter()
            .any(|method| method.name.starts_with("<lambda>"))
    );
    assert!(function(&analysis).cpg.nodes.iter().any(|node| {
        node.kind == "METHOD_REF" && node.method_full_name.as_deref() == Some("source:int()")
    }));
}

#[test]
fn malformed_immediate_lambda_arguments_keep_problem_recovery() {
    let analysis = parse("int f(int n) { return n > [n](int i) { return n + i; }(n+); }");
    assert!(
        function(&analysis)
            .cpg
            .nodes
            .iter()
            .all(|node| node.kind != "CALL")
    );
}

#[test]
fn qualified_variable_template_subscript_keeps_the_index_evaluation() {
    let analysis = parse(
        "namespace demo { template<class T, int N> constexpr int values[] = {N, N + 1}; }
         int tick(); int sink(int);
         template<class T> int f() { return sink(demo::values<T, 0>[tick()]); }
         int after() { return 1; }",
    );
    let f = function(&analysis);
    let index = call(f, "<operator>.indirectIndexAccess");
    let sink = call(f, "sink");
    assert_eq!(argument_code(f, index, 1), "demo::values<T, 0>");
    assert_eq!(argument_code(f, index, 2), "tick()");
    assert_eq!(argument_code(f, sink, 1), "demo::values<T, 0>[tick()]");
    assert!(path(f, call(f, "tick"), index));
    assert!(path(f, index, sink));
    assert_eq!(f.end_line, 3);
    assert!(
        analysis
            .functions
            .iter()
            .any(|f| f.name == "after" && f.start_line == 4)
    );
    assert!(
        analysis
            .functions
            .iter()
            .all(|f| !f.name.starts_with("<lambda>"))
    );
}

#[test]
fn explicitly_qualified_subscript_keeps_receiver_and_argument_effects() {
    let a = parse(
        "namespace demo { struct Box { int operator[](int); }; } demo::Box &receiver(); int tick(); int f() { return receiver().demo::Box::operator[](tick()); }",
    );
    let f = function(&a);
    let id = call(f, "operator[]");
    assert_eq!(argument_code(f, id, 0), "receiver()");
    assert_eq!(argument_code(f, id, 1), "tick()");
    assert!(path(f, call(f, "receiver"), call(f, "tick")));
    assert!(path(f, call(f, "tick"), id));
}

#[test]
fn explicit_member_qualification_does_not_retarget_receiver_method() {
    let a = parse(
        "struct Base { int get(int x) { return x; } }; struct Derived : Base { int get(int x) { return x+1; } }; int f(Derived &x) { return x.Base::get(1); }",
    );
    let f = function(&a);
    let id = call(f, "get");
    assert!(f.cpg.nodes[id as usize].method_full_name.is_none());
    assert_eq!(f.cpg.nodes[id as usize].code, "x.Base::get(1)");
    assert_eq!(argument_code(f, id, 0), "x");
}

#[test]
fn explicit_operator_ids_do_not_become_binary_operators_or_index_expressions() {
    for (name, context, body) in [
        (
            "operator+",
            "namespace demo { struct Object {}; Object operator+(Object,Object); }",
            "demo::operator+(x,x)",
        ),
        (
            "operator[]",
            "namespace demo { struct Object { int operator[](int); }; }",
            "x.operator[](1)",
        ),
        (
            "operator()",
            "namespace demo { struct Object { int operator()(int); }; }",
            "x.operator()(1)",
        ),
    ] {
        let a = parse(&format!(
            "{context} int f(demo::Object x) {{ {body}; return 0; }}"
        ));
        let f = function(&a);
        call(f, name);
        assert!(!f.cpg.nodes.iter().any(|n| matches!(
            n.name.as_deref(),
            Some("<operator>.addition" | "<operator>.indirectIndexAccess")
        )));
    }
}

#[test]
fn comparisons_with_bound_identifiers_are_not_template_calls() {
    let a = parse("int f(int a,int b,int c) { return a < b > (c); }");
    let f = function(&a);
    call(f, "<operator>.lessThan");
    call(f, "<operator>.greaterThan");
}

#[test]
fn template_id_temporaries_are_constructed_and_called() {
    for (temporary, body) in [
        ("less_equal<int>", "return std::less_equal<int>()(x, 1);"),
        (
            "less<std::pair<int,int>>",
            "return std::less<std::pair<int,int>>()(x, 1);",
        ),
        (
            "less_equal<int>",
            "if (x && std::less_equal<int>()(x, 1)) return 1; return 0;",
        ),
    ] {
        let a = parse(&format!("int f(int x) {{ {body} }}"));
        let f = function(&a);
        assert!(
            path(f, call(f, temporary), call(f, "<operator>()")),
            "{body}"
        );
    }
}

#[test]
fn template_parameter_class_and_defaults_do_not_hide_real_record_body() {
    let a = parse(
        "template<class T = int> struct Box { static int get(int x) { return x; } }; int f(int x) { return Box<int>::get(x); }",
    );
    assert!(a.functions.iter().any(|f| f.name == "get"));
    call(function(&a), "get");
}

#[test]
fn template_defaults_keep_member_definitions_and_following_scope() {
    let analysis = parse(
        "namespace demo {
struct Printer {
    template<class T, class N = typename T::size_type>
    int print(N n) { if (n) return 1; return 0; }
    int following(int x) { return x + 1; }
};
}
int after(int x) { return x - 1; }",
    );
    assert_eq!(analysis.functions.len(), 3);
    // PR #49 anchors template definitions after their template heads.
    for (name, qualified, start, end) in [
        ("print", "demo.Printer.print:int(", 4, 4),
        ("following", "demo.Printer.following:int(", 5, 5),
        ("after", "after:int(", 8, 8),
    ] {
        let f = analysis.functions.iter().find(|f| f.name == name).unwrap();
        assert!(f.fullname.starts_with(qualified), "{}", f.fullname);
        assert_eq!((f.start_line, f.end_line), (start, end));
        assert!(f.cpg.nodes.iter().any(|node| node.kind == "RETURN"));
    }
}

#[test]
fn template_default_operators_stay_inside_the_head() {
    let analysis = parse(
        "template<int N = (8 >> 1)> int f(int x) { return x + N; }
         template<class T = int> T object = f<T{}>(0);",
    );
    assert_eq!(analysis.functions.len(), 1);
    let f = function(&analysis);
    assert_eq!(f.return_type, "int");
    let addition = call(f, "<operator>.addition");
    assert_eq!(argument_code(f, addition, 2), "N");
}

#[test]
fn template_body_recovery_keeps_following_definition() {
    let source = r#"struct View { int size() const; };
namespace detail {
template<class T, int> constexpr int values[] = {1};
bool equal(int);
template<class T = int>
int run(View value, int p) {
    {
        for (int first = 0; first < value.size(); ++first) {
            if (detail::equal(detail::values<T, 0>[0])) {
                (void) detail::values<T, 0>[0];
            }
        }
    }
    return 0;
}
}
int after() { return 1; }"#;
    let analysis = analyze(
        source,
        "recovery.cpp",
        &Options {
            language: Some("cpp".into()),
            preprocessed: true,
            ..Options::default()
        },
    )
    .unwrap();
    // PR #38 recorded this dependent array subscript as unsupported; PR #40
    // parses qualified variable template subscripts, so it no longer needs
    // recovery. The template body must still end at its own brace and the
    // later method must keep its own extent.
    assert!(
        analysis.diagnostics.is_empty(),
        "{:?}",
        analysis.diagnostics
    );
    let run = analysis.functions.iter().find(|f| f.name == "run").unwrap();
    // PR #49 anchors template definitions after their template heads.
    assert_eq!((run.start_line, run.end_line), (6, 15));
    let after = analysis
        .functions
        .iter()
        .find(|f| f.name == "after")
        .unwrap();
    assert_eq!((after.start_line, after.end_line), (17, 17));
    assert!(after.cpg.nodes.iter().any(|node| node.kind == "RETURN"));
}

#[test]
fn invalid_qualified_names_keep_existing_strict_errors() {
    for source in [
        "int f(int x) { return ::0x1234::call(x); }",
        "int f(int x) { return demo::Box<int>::(x); }",
    ] {
        assert!(
            analyze(
                source,
                "bad.cpp",
                &Options {
                    language: Some("cpp".into()),
                    strict: true,
                    preprocessed: true,
                    ..Options::default()
                }
            )
            .is_err(),
            "accepted {source}"
        );
    }
}

#[test]
fn unrelated_local_bindings_do_not_shadow_qualified_template_components() {
    let a = parse(
        "namespace demo { template<class T> struct Box { static int get(int); }; struct Object { template<class T> int get(int); }; } int f(demo::Object &x,int get,int Box) { return demo::Box<int>::get(Box) + x.template get<int>(get); }",
    );
    let f = function(&a);
    let calls: Vec<_> = f
        .cpg
        .nodes
        .iter()
        .filter(|n| {
            n.kind == "CALL"
                && (n.code == "demo::Box<int>::get(Box)" || n.code == "x.template get<int>(get)")
        })
        .collect();
    assert_eq!(calls.len(), 2);
    for n in calls {
        assert!(!n.name.as_deref().unwrap().starts_with("<operator>"));
    }
}

#[test]
fn comparison_does_not_capture_a_later_shift_and_cast_as_template_arguments() {
    let a = parse(
        "int f(int x,int y) { int result; result = x < 0; if (y) { x = y >> (unsigned long long)x; } return result+x; }",
    );
    let f = function(&a);
    call(f, "<operator>.lessThan");
    call(f, "<operator>.arithmeticShiftRight");
    assert!(
        f.cpg
            .nodes
            .iter()
            .any(|n| n.kind == "RETURN" && n.code == "return result+x;")
    );
}

#[test]
fn explicit_operator_template_arguments_preserve_calls_and_returns() {
    let context = "struct Box { int value; }; template<class T> bool operator!=(const T &a,const T &b) { return a.value != b.value; }";
    for spelling in ["operator!=<>", "operator!=<Box>", "::operator!=<Box>"] {
        let a = parse(&format!(
            "{context} int f(Box a,Box b) {{ return {spelling}(a,b); }}"
        ));
        let f = function(&a);
        let id = call(f, spelling.trim_start_matches("::"));
        assert_eq!(argument_code(f, id, 1), "a");
        assert_eq!(argument_code(f, id, 2), "b");
        let returns: Vec<_> = f.cpg.nodes.iter().filter(|n| n.kind == "RETURN").collect();
        assert_eq!(returns.len(), 1);
        assert!(path(f, id, returns[0].id));
        assert!(!f.cpg.nodes.iter().any(|n| n.kind == "UNKNOWN"));
    }
}

#[test]
fn operator_template_call_in_comma_condition_keeps_executable_operands() {
    let a = parse(
        "struct Box { int value; }; template<class T> bool operator!=(const T &a,const T &b) { return a.value != b.value; } int touch(); int f(Box a,Box b) { int x; while ((x = operator!=<>(a,b), x && touch())) { a.value--; } return x; }",
    );
    let f = function(&a);
    let comparison = call(f, "operator!=<>");
    let assignment = call(f, "<operator>.assignment");
    assert!(path(f, comparison, assignment));
    assert!(path(f, assignment, call(f, "touch")));
    call(f, "<operator>.logicalAnd");
    assert!(!f.cpg.nodes.iter().any(|n| n.kind == "UNKNOWN"));
}

#[test]
fn qualified_and_member_operator_templates_keep_nested_arguments() {
    for (source, spelling, argument) in [
        (
            "namespace demo { template<class T> struct Box { int value; }; template<class T> int operator+(Box<T> a,Box<T> b) { return a.value+b.value; } } int f(demo::Box<int> a,demo::Box<int> b) { return demo::operator+<int>(a,b); }",
            "operator+<int>",
            "a",
        ),
        (
            "struct Box { template<class T> int operator()(int x) { return x; } }; int f(Box &box,int x) { return box.template operator()<int>(x); }",
            "operator()<int>",
            "x",
        ),
        (
            "template<class T> struct Box { int value; }; template<class T> bool operator!=(T a,T b) { return a.value!=b.value; } int f(Box<Box<int>> a,Box<Box<int>> b) { return operator!=<Box<Box<int>>>(a,b); }",
            "operator!=<Box<Box<int>>>",
            "a",
        ),
    ] {
        let a = parse(source);
        let f = function(&a);
        let id = call(f, spelling);
        assert_eq!(argument_code(f, id, 1), argument);
        assert!(f.cpg.nodes.iter().any(|n| n.kind == "RETURN"));
        assert!(!f.cpg.nodes.iter().any(|n| n.kind == "UNKNOWN"));
    }
}

#[test]
fn operator_call_template_accepts_empty_runtime_arguments() {
    let a = parse(
        "struct Box { template<class T> int operator()(); }; int f(Box &box) { return box.template operator()<int>(); }",
    );
    let f = function(&a);
    call(f, "operator()<int>");
    assert!(f.cpg.nodes.iter().any(|n| n.kind == "RETURN"));
    assert!(!f.cpg.nodes.iter().any(|n| n.kind == "UNKNOWN"));
}

#[test]
fn scopes_inside_operator_template_arguments_do_not_rename_the_callee() {
    let a = parse(
        "namespace types { struct Tag { int value; }; } namespace demo { template<class T> int operator+(T a,T b) { return a.value+b.value; } } int f(types::Tag a,types::Tag b) { return demo::operator+<types::Tag>(a,b); }",
    );
    let f = function(&a);
    let id = call(f, "operator+<types::Tag>");
    assert_eq!(argument_code(f, id, 1), "a");
    assert_eq!(argument_code(f, id, 2), "b");
}

#[test]
fn operator_template_names_do_not_hide_invalid_runtime_arguments() {
    for source in [
        "int f(int a,int b) { return operator!=<>(a,,b); }",
        "int f(int a,int b) { return operator!=<>(a +,b); }",
        "int f(int a,int b) { return operator!=<>(a @,b); }",
    ] {
        let parsed = analyze(
            source,
            "bad.cpp",
            &Options {
                language: Some("cpp".into()),
                strict: true,
                preprocessed: true,
                ..Options::default()
            },
        );
        if let Ok(analysis) = parsed {
            let f = function(&analysis);
            assert!(
                !f.cpg
                    .nodes
                    .iter()
                    .any(|n| n.kind == "CALL" && n.name.as_deref() == Some("operator!=<>")),
                "retained malformed call in {source}"
            );
        }
    }
}

// Without headers CDT binds neither `std::vector` nor `std::string` and reads
// `(std::vector < std::string) > lines`. The default keeps the declaration.
#[test]
fn unknown_template_declaration_is_a_comparison_only_under_cdt_failures() {
    let source = "void f(std::vector<std::string> other) {
        std::vector<std::string> lines; std::vector<std::string> copy = other; }";
    for cdt_failures in [false, true] {
        let analysis = analyze(
            source,
            "unknown.cpp",
            &Options {
                language: Some("cpp".into()),
                strict: true,
                preprocessed: true,
                joern_cdt_failures: cdt_failures,
                ..Options::default()
            },
        )
        .unwrap();
        let f = function(&analysis);
        let count = |kind: &str, name: &str| {
            f.cpg
                .nodes
                .iter()
                .filter(|n| n.kind == kind && n.name.as_deref() == Some(name))
                .count()
        };
        let (locals, comparisons) = if cdt_failures { (0, 2) } else { (1, 0) };
        assert_eq!(count("LOCAL", "lines"), locals);
        assert_eq!(count("LOCAL", "copy"), locals);
        assert_eq!(count("CALL", "<operator>.lessThan"), comparisons);
        assert_eq!(count("CALL", "<operator>.greaterThan"), comparisons);
        assert_eq!(count("CALL", "<operator>.assignment"), 1);
    }
}

#[test]
fn template_member_types_keep_functions_and_parameter_names() {
    let analysis = parse(
        "template<class T> struct Box { typedef T *cursor; }; struct S { void touch(Box<int>::cursor at); int f(Box<int>::cursor at, int n); }; Box<int>::cursor last(int n); Box<int>::cursor first(int n) { return 0; } int S::f(Box<int>::cursor at, int n) { if (n) { touch(at); } return n; }",
    );
    for name in ["last", "first", "f"] {
        assert!(analysis.functions.iter().any(|f| f.name == name), "{name}");
    }
    let f = function(&analysis);
    let parameter = f
        .cpg
        .nodes
        .iter()
        .find(|n| n.kind == "METHOD_PARAMETER_IN" && n.code.starts_with("Box"))
        .unwrap();
    assert_eq!(parameter.name.as_deref(), Some("at"));
    assert_eq!(argument_code(f, call(f, "touch"), 1), "at");
}

#[test]
fn pointer_type_argument_before_comma_keeps_qualified_call_and_return() {
    let analysis = parse(
        "template<class T, class U> struct Box { static int get(T); };
        template<class T> T* tick(T*);
        template<class T, class U> int f(T* value) {
            return Box<const T*, U>::get(tick(value));
        }",
    );
    let f = function(&analysis);
    let get = call(f, "get");
    assert_eq!(argument_code(f, get, 1), "tick(value)");
    assert!(path(f, call(f, "tick"), get));
    assert!(f.cpg.nodes.iter().any(|n| n.kind == "RETURN"));
}

#[test]
fn missing_operand_in_qualified_call_argument_stays_a_problem() {
    let analysis = parse(
        "template<class T, class U> struct Box { static int get(int, int); };
        int f(int value) {
            return Box<const int*, int>::get(value *, 2);
        }",
    );
    let f = function(&analysis);
    assert!(f.cpg.nodes.iter().all(|n| n.kind != "RETURN"));
    assert!(f.cpg.nodes.iter().all(|n| n.name.as_deref() != Some("get")));
}

#[test]
fn explicit_member_template_specializations_keep_bodies() {
    // Joern 4.0.150 exports `C::StringTo<double>` as the method `StringTo`.
    let source = "int tick(int);\nstruct C {\n  template <class T> T StringTo(const char *b, int n) const;\n};\ntemplate<>\ndouble C::StringTo<double>(const char *b, int n) const {\n  while (n > 0) n = tick(n);\n  return n;\n}\n";
    let analysis = rust_joern::analyze(
        source,
        "member.cpp",
        &rust_joern::graph::Options {
            language: Some("cpp".into()),
            preprocessed: true,
            strict: true,
            ..Default::default()
        },
    )
    .unwrap();
    let method = analysis
        .functions
        .iter()
        .find(|f| f.name == "StringTo" && f.start_line < f.end_line)
        .expect("specialized member body");
    assert!(method.cfg.edges.len() >= 3);
}

#[test]
fn member_comparison_does_not_consume_the_following_operator_definition() {
    let a = parse(
        "struct Node { const int *value; bool operator<(const Node&) const; bool operator>(const Node&) const; };\n\
         bool Node::operator<(const Node& other) const { return (value < other.value); }\n\
         bool Node::operator>(const Node& other) const { return (value > other.value); }",
    );
    for (line, op, code) in [
        (2, "<operator>.lessThan", "return (value < other.value);"),
        (3, "<operator>.greaterThan", "return (value > other.value);"),
    ] {
        let method = a.functions.iter().find(|f| f.start_line == line).unwrap();
        let comparison = call(method, op);
        let returned = method
            .cpg
            .nodes
            .iter()
            .find(|node| node.kind == "RETURN" && node.code == code)
            .unwrap();
        assert!(path(method, comparison, returned.id));
    }
}

#[test]
fn template_call_keeps_comparisons_and_nested_closers_inside_the_name() {
    let a = parse(
        "namespace demo { template<bool B> struct Flag {}; template<class T> int get(int); }\n\
         int f() { return demo::get<demo::Flag<(2 > 1)>>(0); }",
    );
    let f = function(&a);
    let template = f
        .cpg
        .nodes
        .iter()
        .find(|node| node.kind == "CALL" && node.code == "demo::get<demo::Flag<(2 > 1)>>(0)")
        .unwrap();
    let returned = f
        .cpg
        .nodes
        .iter()
        .find(|node| node.kind == "RETURN")
        .unwrap();
    assert!(path(f, template.id, returned.id));
    assert!(!f.cpg.nodes.iter().any(|node| matches!(
        node.name.as_deref(),
        Some("<operator>.lessThan" | "<operator>.greaterThan" | "<operator>.arithmeticShiftRight")
    )));
}

#[test]
fn single_angle_before_a_shift_remains_a_comparison() {
    let a = parse(
        "struct Node { int value; }; int f(Node n, int x, int y) { return n.value < x >> (y); }",
    );
    let f = function(&a);
    let shifted = call(f, "<operator>.arithmeticShiftRight");
    let compared = call(f, "<operator>.lessThan");
    assert!(path(f, shifted, compared));
}

#[test]
fn template_qualified_type_names_preserve_declarations_and_for_flow() {
    let analysis = parse(
        "namespace demo {
            template<class T> struct Outer {
                template<class U> struct Inner { using Iterator = U*; };
            };
         }
         template<class T> struct Vec { using Iterator = T*; };
         struct State { int* begin(); int* end(); };
         void body(int);
         int done();
         int f(State* state) {
             Vec<int>::Iterator last = state->end();
             for (::demo::Outer<int>::Inner<int>::Iterator i = state->begin(); i != last; ++i) {
                 body(*i);
             }
             return done();
         }",
    );
    let f = function(&analysis);
    for name in ["last", "i"] {
        assert!(
            f.cpg
                .nodes
                .iter()
                .any(|node| { node.kind == "LOCAL" && node.name.as_deref() == Some(name) }),
            "missing declaration of {name}"
        );
    }
    let condition = call(f, "<operator>.notEquals");
    let body = call(f, "body");
    let increment = call(f, "<operator>.preIncrement");
    assert!(path(f, call(f, "end"), call(f, "begin")));
    assert!(path(f, call(f, "begin"), condition));
    assert!(path(f, condition, body));
    assert!(path(f, body, increment));
    assert!(path(f, increment, condition));
    assert!(path(f, condition, call(f, "done")));
}

#[test]
fn parenthesized_template_qualified_callee_stays_an_expression() {
    let analysis = parse(
        "template<class T> struct Box { static int value(); };
         int f() { return (Box<int>::value)(); }",
    );
    let f = function(&analysis);
    let invoked = call(f, "<operator>()");
    let ret = f
        .cpg
        .nodes
        .iter()
        .find(|node| node.kind == "RETURN")
        .unwrap();
    assert!(path(f, invoked, ret.id));
}

#[test]
fn placement_new_preserves_operands_in_all_expression_positions() {
    let context = "namespace demo { struct Object { Object(int); }; } void *location(); int value(); void consume(demo::Object *);";
    for body in [
        "new (location()) demo::Object(value());",
        "return new (location()) demo::Object(value());",
        "consume(new (location()) demo::Object(value()));",
        "object = new (location()) demo::Object(value());",
    ] {
        let analysis = parse(&format!(
            "{context} demo::Object *f(demo::Object *object) {{ {body} }}"
        ));
        let f = function(&analysis);
        let allocation = call(f, "<operator>.new");
        assert_eq!(argument_code(f, allocation, 1), "demo::Object");
        assert_eq!(argument_code(f, allocation, 2), "location()");
        assert_eq!(argument_code(f, allocation, 3), "value()");
        assert!(path(f, call(f, "location"), call(f, "value")));
        assert!(path(f, call(f, "value"), allocation));
        if body.starts_with("return") {
            let returned = f.cpg.nodes.iter().find(|n| n.kind == "RETURN").unwrap();
            assert!(path(f, allocation, returned.id));
        }
    }
}

#[test]
fn placement_new_as_unbraced_body_keeps_the_control_statement() {
    let context = "struct Item { Item(int); }; void after();";
    for statement in [
        "if (count) new (items + count) Item(count);",
        "while (count--) new (items + count) Item(count);",
        "for (int i = 0; i < count; i++) new (items + i) Item(i);",
    ] {
        let analysis = parse(&format!(
            "{context} void f(Item *items, int count) {{ {statement} after(); }}"
        ));
        let f = function(&analysis);
        let allocation = call(f, "<operator>.new");
        let after = call(f, "after");
        assert!(f.cpg.nodes.iter().any(|n| n.kind == "CONTROL_STRUCTURE"));
        assert!(path(f, allocation, after), "{statement}");
        assert!(
            !f.cpg
                .nodes
                .iter()
                .any(|n| n.code == "Item(count)" || n.code == "Item(i)")
        );
    }
}

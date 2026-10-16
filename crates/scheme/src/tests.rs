use super::*;

fn run(src: &str) -> String {
    let mut i = Interp::new();
    match i.eval_str(src) {
        Ok(v) => v.write(),
        Err(e) => format!("ERR: {e}"),
    }
}

#[test]
fn arithmetic_keeps_integers_exact_and_promotes_on_overflow() {
    assert_eq!(run("(+ 1 2 3)"), "6");
    assert_eq!(run("(/ 6 3)"), "2");
    assert_eq!(run("(/ 1 4)"), "0.25");
    assert_eq!(run("(* 99999999999 99999999999)"), "9999999999800000000000");
    assert_eq!(run("(- 5)"), "-5");
    assert_eq!(run("(modulo -7 3)"), "2");
    assert_eq!(run("(remainder -7 3)"), "-1");
    assert_eq!(run("(sqrt 16)"), "4");
    assert_eq!(run("(expt 2 10)"), "1024");
    assert_eq!(run("(round 2.5)"), "2.0");
    assert_eq!(run("(max 1 2.0)"), "2.0");
    assert_eq!(run("(/ 1 0)"), "ERR: division by zero");
}

#[test]
fn definitions_closures_and_tail_calls() {
    assert_eq!(run("(define (fact n) (if (= n 0) 1 (* n (fact (- n 1))))) (fact 10)"), "3628800");
    assert_eq!(run("(define (count n acc) (if (= n 0) acc (count (- n 1) (+ acc 1)))) (count 200000 0)"), "200000");
    assert_eq!(run("(define (make-adder n) (lambda (x) (+ x n))) ((make-adder 3) 4)"), "7");
    assert_eq!(run("(define x 1) (set! x (+ x 1)) x"), "2");
    assert_eq!(run("(define (f a . rest) (list a rest)) (f 1 2 3)"), "(1 (2 3))");
}

#[test]
fn binding_forms_and_control() {
    assert_eq!(run("(let ((a 1) (b 2)) (+ a b))"), "3");
    assert_eq!(run("(let* ((a 1) (b (+ a 1))) (* a b))"), "2");
    assert_eq!(run("(let loop ((i 0) (acc '())) (if (= i 3) (reverse acc) (loop (+ i 1) (cons i acc))))"), "(0 1 2)");
    assert_eq!(run("(letrec ((even? (lambda (n) (if (= n 0) #t (odd? (- n 1))))) (odd? (lambda (n) (if (= n 0) #f (even? (- n 1)))))) (even? 100))"), "#t");
    assert_eq!(run("(do ((i 0 (+ i 1)) (s 0 (+ s i))) ((= i 5) s))"), "10");
    assert_eq!(run("(cond ((> 1 2) 'a) ((assv 2 '((1 . one) (2 . two))) => cdr) (else 'c))"), "two");
    assert_eq!(run("(case 3 ((1 2) 'low) ((3 4) 'mid) (else 'high))"), "mid");
    assert_eq!(run("(and 1 2 #f 3)"), "#f");
    assert_eq!(run("(or #f 7)"), "7");
    assert_eq!(run("(when #t 1 2)"), "2");
}

#[test]
fn lists_strings_vectors() {
    assert_eq!(run("(map + '(1 2 3) '(10 20 30))"), "(11 22 33)");
    assert_eq!(run("(append '(1) '(2 3) '(4))"), "(1 2 3 4)");
    assert_eq!(run("(list-tail '(1 2 3 4) 2)"), "(3 4)");
    assert_eq!(run("(assq 'b '((a 1) (b 2)))"), "(b 2)");
    assert_eq!(run("(apply + 1 2 '(3 4))"), "10");
    assert_eq!(run("(filter odd? (iota 6))"), "(1 3 5)");
    assert_eq!(run("(fold-left - 10 '(1 2 3))"), "4");
    assert_eq!(run("(string-append \"ab\" \"cd\")"), "\"abcd\"");
    assert_eq!(run("(substring \"hello\" 1 3)"), "\"el\"");
    assert_eq!(run("(strbreakup \"a,b,c\" \",\")"), "(\"a\" \"b\" \"c\")");
    assert_eq!(run("(unbreakupstr '(\"a\" \"b\") \"-\")"), "\"a-b\"");
    assert_eq!(run("(number->string 255 16)"), "\"ff\"");
    assert_eq!(run("(string->number \"3.5\")"), "3.5");
    assert_eq!(run("(let ((v (make-vector 3 0))) (vector-set! v 1 9) v)"), "#(0 9 0)");
    assert_eq!(run("(let ((a (cons-array 2 'byte))) (aset a 0 5) (aref a 0))"), "5");
    assert_eq!(run("`(1 ,(+ 1 1) ,@(list 3 4))"), "(1 2 3 4)");
}

#[test]
fn output_is_collected() {
    let mut i = Interp::new();
    i.eval_str("(display \"hi \") (write \"x\") (newline) (display '(1 \"a\" #\\b))").unwrap();
    assert_eq!(i.take_output(), "hi \"x\"\n(1 a b)");
}

#[test]
fn errors_are_values_not_panics() {
    for src in [
        "(car '())",
        "(vector-ref (vector 1) 5)",
        "(undefined-thing)",
        "(+ 'a 1)",
        "((lambda (x) x))",
        "(error \"boom\" 1)",
        "(",
        ")",
        "\"open",
        "(define)",
        "(substring \"abc\" 2 1)",
        "(string-ref \"a\" -1)",
        "(list-ref '(1) 9)",
        "(make-vector -1)",
        "(make-string 999999999999)",
        "(iota 99999999999)",
        "#\\nonsense",
        "(random 0)",
    ] {
        assert!(run(src).starts_with("ERR"), "{src} -> {}", run(src));
    }
    assert_eq!(run("(error \"boom\" 1)"), "ERR: boom 1");
}

#[test]
fn runaway_scripts_hit_the_limits() {
    let mut i = Interp::new();
    i.max_steps = 100_000;
    assert_eq!(i.eval_str("(define (loop) (loop)) (loop)").unwrap_err().0, "script exceeded its step budget");
    let mut i = Interp::new();
    assert_eq!(i.eval_str("(define (down n) (+ 1 (down (+ n 1)))) (down 0)").unwrap_err().0, "recursion too deep");
    // Deeply nested data and long lists neither overflow the stack nor hang.
    assert!(run(&format!("{}1{}", "(".repeat(5000), ")".repeat(5000))).starts_with("ERR"));
    assert_eq!(run("(length (iota 200000))"), "200000");
}

struct Pdb;
impl Host for Pdb {
    fn constant(&mut self, name: &str) -> Option<Value> {
        (name == "RGB").then_some(Value::Int(0))
    }
    fn call(&mut self, interp: &mut Interp, name: &str, args: &[Value]) -> Option<Result<Value>> {
        match name {
            "gimp-double" => Some(args.first().and_then(Value::as_int).map(|n| Value::Int(n * 2)).ok_or_else(|| Error::msg("gimp-double: bad argument"))),
            "gimp-call-back" => {
                let f = args.first()?.clone();
                Some(interp.apply_value(&f, vec![Value::Int(20)]))
            }
            _ => None,
        }
    }
}

#[test]
fn host_procedures_and_constants() {
    let mut i = Interp::new();
    i.set_host(Box::new(Pdb));
    assert_eq!(i.eval_str("(gimp-double 21)").unwrap().write(), "42");
    assert_eq!(i.eval_str("(list RGB TRUE FALSE)").unwrap().write(), "(0 #t #f)");
    assert_eq!(i.eval_str("(gimp-call-back (lambda (n) (+ n 1)))").unwrap().write(), "21");
    assert_eq!(i.eval_str("(gimp-double 'x)").unwrap_err().0, "gimp-double: bad argument");
    assert_eq!(i.eval_str("(gimp-nothing)").unwrap_err().0, "unbound variable: gimp-nothing");
    // A script's own definition shadows the host.
    assert_eq!(i.eval_str("(define (gimp-double x) 0) (gimp-double 21)").unwrap().write(), "0");
}

#[test]
fn deterministic_random() {
    let mut a = Interp::new();
    let mut b = Interp::new();
    a.seed_random(7);
    b.seed_random(7);
    let (x, y) = (a.eval_str("(random 1000)").unwrap().write(), b.eval_str("(random 1000)").unwrap().write());
    assert_eq!(x, y);
}

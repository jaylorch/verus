#![feature(rustc_private)]
#[macro_use]
mod common;
use common::*;

test_verify_one_file! {
    #[test] test1 verus_code! {
        struct S {}

        impl S {
            spec fn f(&self) -> bool { true }

            // Note: it's a good idea for g and f to return the same value,
            // but, for testing purposes only, we have them return different values here.
            #[verifier::when_used_as_spec(f)]
            fn g(&self) -> bool { false }

            fn h(&self) {
                self.g(); // call g
                assert(self.g()); // in a spec, so call f, not g
            }
        }
    } => Ok(())
}

test_verify_one_file! {
    #[test] fail_exec_false verus_code! {
        struct S {}

        impl S {
            spec fn f(&self) -> bool { true }

            #[verifier::when_used_as_spec(f)]
            fn g(&self) -> bool { false }

            fn h(&self) {
                let b = self.g();
                assert(b); // FAILS
            }
        }
    } => Err(err) => assert_one_fails(err)
}

test_verify_one_file! {
    #[test] fail_different_typarg verus_code! {
        struct S {}

        impl S {
            spec fn f<A>(&self, k: &A) -> bool { true }

            #[verifier::when_used_as_spec(f)]
            fn g<B>(&self, k: &B) -> bool { true }
        }
    } => Err(err) => assert_vir_error_msg(err, "when_used_as_spec function should have the same type bounds")
}

test_verify_one_file! {
    #[test] fail_different_arg verus_code! {
        struct S {}

        impl S {
            spec fn f(&self, k: int) -> bool { true }

            #[verifier::when_used_as_spec(f)]
            fn g(&self, k: u64) -> bool { true }
        }
    } => Err(err) => assert_vir_error_msg(err, "when_used_as_spec function should have the same parameters")
}

test_verify_one_file! {
    #[test] fail_different_returns verus_code! {
        struct S {}

        impl S {
            spec fn f(&self) -> bool { true }

            #[verifier::when_used_as_spec(f)]
            fn g(&self) -> u8 { 0 }
        }
    } => Err(err) => assert_vir_error_msg(err, "when_used_as_spec function should have the same return types")
}

test_verify_one_file! {
    #[test] fail_not_exec verus_code! {
        struct S {}

        impl S {
            spec fn f(&self, k: u64) -> bool { true }

            #[verifier::when_used_as_spec(f)]
            proof fn g(&self, k: u64) -> bool { true }
        }
    } => Err(err) => assert_vir_error_msg(err, "when_used_as_spec must point from an exec function to a spec function")
}

test_verify_one_file! {
    #[test] fail_not_spec verus_code! {
        struct S {}

        impl S {
            proof fn f(&self, k: u64) -> bool { true }

            #[verifier::when_used_as_spec(f)]
            fn g(&self, k: u64) -> bool { true }
        }
    } => Err(err) => assert_vir_error_msg(err, "when_used_as_spec must point from an exec function to a spec function")
}

test_verify_one_file! {
    #[test] fail_missing verus_code! {
        struct S {}

        impl S {
            #[verifier::when_used_as_spec(f)]
            fn g(&self, k: u64) -> bool { true }
        }
    } => Err(err) => assert_vir_error_msg(err, "cannot find function referred to in when_used_as_spec")
}

test_verify_one_file! {
    #[test] fail_traits_decl verus_code! {
        spec fn f() -> bool { true }

        trait T {
            #[verifier::when_used_as_spec(f)]
            fn g() -> bool { false }
        }
    } => Err(err) => assert_vir_error_msg(err, "cannot find function referred to in when_used_as_spec")
}

test_verify_one_file! {
    #[test] test_traits verus_code! {
        use vstd::prelude::*;
        trait T {
            spec fn f() -> bool;

            #[verifier::when_used_as_spec(f)]
            fn g() -> bool;
        }

        uninterp spec fn h() -> bool;

        impl T for bool {
            uninterp spec fn f() -> bool;

            #[verifier::when_used_as_spec(h)]
            fn g() -> bool { true }
        }

        proof fn test<A: T>() {
            assert(A::g() == A::f());
            assert(<bool as T>::g() == h());
            assert(<bool as T>::g() == <bool as T>::f()); // FAILS
        }
    } => Err(err) => assert_one_fails(err)
}

test_verify_one_file! {
    #[test] test_traits2 verus_code! {
        // https://github.com/verus-lang/verus/issues/1767
        trait T {
            spec fn f() -> bool;

            #[verifier::when_used_as_spec(f)]
            fn g() -> bool
                returns Self::f();

            proof fn lemma()
                ensures Self::g();
        }

        struct S;
        impl T for S {
            spec fn f() -> bool { true }

            fn g() -> bool { true }

            proof fn lemma()
                ensures Self::g()
            {}
        }

        proof fn test() {
            assert(S::f() == S::g());
        }
    } => Ok(())
}

test_verify_one_file! {
    #[test] vec_len_regression_issue212 verus_code! {
        use vstd::prelude::*;

        struct S {
            pub vec: Vec<()>,
        }

        impl S {
            spec fn f(&self) -> bool {
                0 < self.vec.len()
            }
        }

        fn test() {
            let mut s = S { vec: Vec::new() };
            assert(!s.f());
            s.vec.push(());
            assert(s.f());
        }
    } => Ok(())
}

test_verify_one_file! {
    #[test] visibility verus_code! {
        mod X {
            spec fn spec_not(x: bool) -> bool { !x }

            #[verifier::when_used_as_spec(spec_not)]
            pub fn exec_not(x: bool) -> (res: bool)
            {
                !x
            }
        }
    } => Err(err) => assert_vir_error_msg(err, "when_used_as_spec refers to function which is more private")
}

test_verify_one_file! {
    #[test] allow_in_spec verus_code! {
        #[verifier::allow_in_spec]
        fn negate(a: bool) -> bool
            returns !a
        {
            !a
        }

        proof fn test(b: bool) {
            assert(negate(b) == !b);
        }

        uninterp spec fn arbitrary<T>() -> T;

        #[verifier::allow_in_spec]
        fn generic<T>(a: bool, b: T) -> T
            returns if a { b } else { arbitrary() }
        {
            if a {
                b
            } else {
                loop decreases 0int { assume(false); }
            }
        }

        proof fn test2(c: u64) {
            assert(generic(true, c) == c);
            assert(generic(false, c) == arbitrary::<u64>());
        }
    } => Ok(())
}

test_verify_one_file! {
    #[test] allow_in_spec_conflict verus_code! {
        pub open spec fn negate_spec(a: bool) -> bool {
            !a
        }

        #[verifier::when_used_as_spec(negate_spec)]
        #[verifier::allow_in_spec]
        pub fn negate(a: bool) -> bool
            returns !a
        {
            !a
        }
    } => Err(err) => assert_vir_error_msg(err, "a function cannot be marked both 'when_used_as_spec' and 'allow_in_spec'")
}

use proxmox_api_macro::api;
use proxmox_router::SharedStateRegistry;

use anyhow::Error;
use serde_json::{Value, json};

#[derive(Clone)]
struct Foo {
    val: i32,
}

#[derive(Clone)]
struct Unregistered;

#[api(
    input: {
        properties: {
            value: {
                description: "Something",
            }
        }
    }
)]
/// Test multiple state args.
fn multiple_args(
    #[state] state_arg1: i32,
    value: isize,
    #[state] state_arg2: Foo,
) -> Result<(), Error> {
    assert_eq!(state_arg1, 10);
    assert_eq!(state_arg2.val, 20);

    assert_eq!(value, 50);

    Ok(())
}

#[api]
/// Test an owned state arg next to a mutable environment.
fn with_rpcenv(
    #[state] state_arg: Foo,
    rpcenv: &mut dyn proxmox_router::RpcEnvironment,
) -> Result<(), Error> {
    let _ = rpcenv;

    assert_eq!(state_arg.val, 20);

    Ok(())
}

#[api]
/// Test optional state args.
fn optional_args(
    #[state] state_arg1: Option<i32>,
    #[state] state_arg3: Option<Unregistered>,
) -> Result<(), Error> {
    assert_eq!(state_arg1, Some(10));
    assert!(state_arg3.is_none());

    Ok(())
}

#[api]
/// Test optional state args without a registry.
fn optional_without_registry(#[state] state_arg1: Option<i32>) -> Result<(), Error> {
    assert!(state_arg1.is_none());

    Ok(())
}

#[api]
/// Return optional state using each standard spelling of Option.
fn qualified_optional_args(
    #[state] bare: Option<i32>,
    #[state] std: std::option::Option<i32>,
    #[state] core: core::option::Option<i32>,
    #[state] absolute_std: ::std::option::Option<i32>,
    #[state] absolute_core: ::core::option::Option<i32>,
) -> Result<Vec<Option<i32>>, Error> {
    Ok(vec![bare, std, core, absolute_std, absolute_core])
}

mod custom {
    #[derive(Clone)]
    pub struct Option<T>(pub T);
}

#[api]
/// Retrieve a custom type named Option without unwrapping it.
fn custom_option(#[state] value: custom::Option<i32>) -> Result<i32, Error> {
    Ok(value.0)
}

#[api]
/// Test not registered state arg.
fn not_registered(#[state] _state: Foo) -> Result<(), Error> {
    panic!("should not reach this");
}

#[api(
    input: {
        properties: {
            unused: {
                description: "foo",
            }
        }
    }
)]
/// Test whether attributes on *other* arguments are preserved.
fn other_attributes_are_preserved(
    #[state] _state_arg: Option<Foo>,
    #[allow(unused)] unused: i32,
) -> Result<(), Error> {
    Ok(())
}

struct RpcEnv {
    shared_state: Option<SharedStateRegistry>,
}
impl proxmox_router::RpcEnvironment for RpcEnv {
    fn result_attrib_mut(&mut self) -> &mut Value {
        panic!("result_attrib_mut called");
    }

    fn result_attrib(&self) -> &Value {
        panic!("result_attrib called");
    }

    /// The environment type
    fn env_type(&self) -> proxmox_router::RpcEnvironmentType {
        panic!("env_type called");
    }

    /// Set authentication id
    fn set_auth_id(&mut self, user: Option<String>) {
        let _ = user;
        panic!("set_auth_id called");
    }

    /// Get authentication id
    fn get_auth_id(&self) -> Option<String> {
        panic!("get_auth_id called");
    }

    fn shared_state(&self) -> Option<&SharedStateRegistry> {
        self.shared_state.as_ref()
    }
}

#[test]
fn test_invocations() {
    let mut shared_state = SharedStateRegistry::default();

    shared_state.register::<i32>(10).unwrap();
    shared_state.register::<Foo>(Foo { val: 20 }).unwrap();

    let mut env = RpcEnv {
        shared_state: Some(shared_state),
    };

    api_function_multiple_args(
        json!({
            "value": 50,
        }),
        &API_METHOD_MULTIPLE_ARGS,
        &mut env,
    )
    .expect("func with multiple injected works");

    api_function_with_rpcenv(json!({}), &API_METHOD_WITH_RPCENV, &mut env)
        .expect("func with injected state and environment works");

    api_function_optional_args(json!({}), &API_METHOD_OPTIONAL_ARGS, &mut env)
        .expect("func with optional injected state works");
}

#[test]
fn test_qualified_optional_args() {
    let mut registered = SharedStateRegistry::default();
    registered.register(10_i32).unwrap();

    for (shared_state, expected) in [
        (None, json!([null, null, null, null, null])),
        (
            Some(SharedStateRegistry::default()),
            json!([null, null, null, null, null]),
        ),
        (Some(registered), json!([10, 10, 10, 10, 10])),
    ] {
        let mut env = RpcEnv { shared_state };
        let result = api_function_qualified_optional_args(
            json!({}),
            &API_METHOD_QUALIFIED_OPTIONAL_ARGS,
            &mut env,
        )
        .unwrap();
        assert_eq!(result, expected);
    }
}

#[test]
fn test_custom_option_is_required_state() {
    let mut registry = SharedStateRegistry::default();
    registry.register(custom::Option(42_i32)).unwrap();
    let mut env = RpcEnv {
        shared_state: Some(registry),
    };
    assert_eq!(
        api_function_custom_option(json!({}), &API_METHOD_CUSTOM_OPTION, &mut env).unwrap(),
        json!(42),
    );

    let mut env = RpcEnv { shared_state: None };
    assert!(api_function_custom_option(json!({}), &API_METHOD_CUSTOM_OPTION, &mut env).is_err());
}

#[test]
fn test_not_registered() {
    let mut env = RpcEnv {
        shared_state: Some(SharedStateRegistry::default()),
    };

    api_function_not_registered(json!({}), &API_METHOD_NOT_REGISTERED, &mut env)
        .expect_err("func did not fail");

    api_function_optional_without_registry(
        json!({}),
        &API_METHOD_OPTIONAL_WITHOUT_REGISTRY,
        &mut env,
    )
    .expect("optional state args are 'None' if not registered");
}

#[test]
fn test_without_registry() {
    let mut env = RpcEnv { shared_state: None };

    api_function_not_registered(json!({}), &API_METHOD_NOT_REGISTERED, &mut env)
        .expect_err("func did not fail");

    api_function_optional_without_registry(
        json!({}),
        &API_METHOD_OPTIONAL_WITHOUT_REGISTRY,
        &mut env,
    )
    .expect("optional state args are 'None' without a registry");
}

#[test]
/// Test whether attributes on *other* arguments are preserved.
///
/// This test does not really assert anything, but the called function
/// will emit a warning during compilation if it does not work.
fn test_attributes_are_preserved() {
    let mut env = RpcEnv { shared_state: None };

    api_function_other_attributes_are_preserved(
        json!({
            "unused": 3
        }),
        &API_METHOD_OTHER_ATTRIBUTES_ARE_PRESERVED,
        &mut env,
    )
    .unwrap();
}

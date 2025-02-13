use super::*;

#[test]
fn unbounded_loop() {
    // Set handler timeout to 3 seconds.
    env::set_var(crate::host::TIMEOUT_ENV_VAR, "3");
    let valid_module = test_valid_module(mock_data_source("wasm_test/non_terminating.wasm"));
    let mut module = WasmiModule::from_valid_module_with_ctx(valid_module, mock_context()).unwrap();
    module.start_time = Instant::now();
    let err = module
        .module
        .clone()
        .invoke_export("loop", &[], &mut module)
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        "Trap: Trap { kind: Host(HostExportError(\"Mapping handler timed out\")) }"
    );
}

#[test]
fn unbounded_recursion() {
    let valid_module = test_valid_module(mock_data_source("wasm_test/non_terminating.wasm"));
    let mut module = WasmiModule::from_valid_module_with_ctx(valid_module, mock_context()).unwrap();
    let err = module
        .module
        .clone()
        .invoke_export("rabbit_hole", &[], &mut module)
        .unwrap_err();
    assert_eq!(err.to_string(), "Trap: Trap { kind: StackOverflow }");
}

#[test]
fn abi_array() {
    let valid_module = test_valid_module(mock_data_source("wasm_test/abi_classes.wasm"));
    let mut module = WasmiModule::from_valid_module_with_ctx(valid_module, mock_context()).unwrap();

    let vec = vec![
        "1".to_owned(),
        "2".to_owned(),
        "3".to_owned(),
        "4".to_owned(),
    ];
    let vec_obj: AscPtr<Array<AscPtr<AscString>>> = module.asc_new(&*vec);

    let new_vec_obj: AscPtr<Array<AscPtr<AscString>>> =
        module.takes_ptr_returns_ptr("test_array", vec_obj);
    let new_vec: Vec<String> = module.asc_get(new_vec_obj);

    assert_eq!(
        new_vec,
        vec![
            "1".to_owned(),
            "2".to_owned(),
            "3".to_owned(),
            "4".to_owned(),
            "5".to_owned()
        ]
    )
}

#[test]
fn abi_subarray() {
    let valid_module = test_valid_module(mock_data_source("wasm_test/abi_classes.wasm"));
    let mut module = WasmiModule::from_valid_module_with_ctx(valid_module, mock_context()).unwrap();

    let vec: Vec<u8> = vec![1, 2, 3, 4];
    let vec_obj: AscPtr<TypedArray<u8>> = module.asc_new(&*vec);

    let new_vec_obj: AscPtr<TypedArray<u8>> =
        module.takes_ptr_returns_ptr("byte_array_third_quarter", vec_obj);
    let new_vec: Vec<u8> = module.asc_get(new_vec_obj);

    assert_eq!(new_vec, vec![3])
}

#[test]
fn abi_bytes_and_fixed_bytes() {
    let valid_module = test_valid_module(mock_data_source("wasm_test/abi_classes.wasm"));
    let mut module = WasmiModule::from_valid_module_with_ctx(valid_module, mock_context()).unwrap();
    let bytes1: Vec<u8> = vec![42, 45, 7, 245, 45];
    let bytes2: Vec<u8> = vec![3, 12, 0, 1, 255];

    let bytes1_ptr = module.asc_new::<Uint8Array, _>(&*bytes1);
    let bytes2_ptr = module.asc_new::<Uint8Array, _>(&*bytes2);
    let new_vec_obj: AscPtr<Uint8Array> =
        module.takes_ptr_ptr_returns_ptr("concat", bytes1_ptr, bytes2_ptr);

    // This should be bytes1 and bytes2 concatenated.
    let new_vec: Vec<u8> = module.asc_get(new_vec_obj);

    let mut concated = bytes1.clone();
    concated.extend(bytes2.clone());
    assert_eq!(new_vec, concated);
}

/// Test a roundtrip Token -> Payload -> Token identity conversion through asc,
/// and assert the final token is the same as the starting one.
#[test]
fn abi_ethabi_token_identity() {
    let valid_module = test_valid_module(mock_data_source("wasm_test/abi_token.wasm"));
    let mut module = WasmiModule::from_valid_module_with_ctx(valid_module, mock_context()).unwrap();

    // Token::Address
    let address = H160([1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1]);
    let token_address = Token::Address(address);

    let token_address_ptr = module.asc_new(&token_address);
    let new_address_obj: AscPtr<ArrayBuffer<u8>> =
        module.takes_ptr_returns_ptr("token_to_address", token_address_ptr);

    let new_token_ptr = module.takes_ptr_returns_ptr("token_from_address", new_address_obj);
    let new_token = module.asc_get(new_token_ptr);

    assert_eq!(token_address, new_token);

    // Token::Bytes
    let token_bytes = Token::Bytes(vec![42, 45, 7, 245, 45]);

    let token_bytes_ptr = module.asc_new(&token_bytes);
    let new_bytes_obj: AscPtr<ArrayBuffer<u8>> =
        module.takes_ptr_returns_ptr("token_to_bytes", token_bytes_ptr);

    let new_token_ptr = module.takes_ptr_returns_ptr("token_from_bytes", new_bytes_obj);
    let new_token = module.asc_get(new_token_ptr);

    assert_eq!(token_bytes, new_token);

    // Token::Int
    let int_token = Token::Int(U256([256, 453452345, 0, 42]));

    let int_token_ptr = module.asc_new(&int_token);
    let new_int_obj: AscPtr<ArrayBuffer<u8>> =
        module.takes_ptr_returns_ptr("token_to_int", int_token_ptr);

    let new_token_ptr = module.takes_ptr_returns_ptr("token_from_int", new_int_obj);
    let new_token = module.asc_get(new_token_ptr);

    assert_eq!(int_token, new_token);

    // Token::Uint
    let uint_token = Token::Uint(U256([256, 453452345, 0, 42]));

    let uint_token_ptr = module.asc_new(&uint_token);
    let new_uint_obj: AscPtr<ArrayBuffer<u8>> =
        module.takes_ptr_returns_ptr("token_to_uint", uint_token_ptr);

    let new_token_ptr = module.takes_ptr_returns_ptr("token_from_uint", new_uint_obj);
    let new_token = module.asc_get(new_token_ptr);

    assert_eq!(uint_token, new_token);
    assert_ne!(uint_token, int_token);

    // Token::Bool
    let token_bool = Token::Bool(true);

    let token_bool_ptr = module.asc_new(&token_bool);
    let boolean: bool = module
        .module
        .clone()
        .invoke_export(
            "token_to_bool",
            &[RuntimeValue::from(token_bool_ptr)],
            &mut module,
        )
        .expect("call failed")
        .expect("call returned nothing")
        .try_into::<bool>()
        .expect("call did not return bool");

    let new_token_ptr =
        module.takes_val_returns_ptr("token_from_bool", RuntimeValue::from(boolean as u32));
    let new_token = module.asc_get(new_token_ptr);

    assert_eq!(token_bool, new_token);

    // Token::String
    let token_string = Token::String("漢字Go🇧🇷".into());

    let token_string_ptr = module.asc_new(&token_string);
    let new_string_obj: AscPtr<AscString> =
        module.takes_ptr_returns_ptr("token_to_string", token_string_ptr);

    let new_token_ptr = module.takes_ptr_returns_ptr("token_from_string", new_string_obj);
    let new_token = module.asc_get(new_token_ptr);

    assert_eq!(token_string, new_token);

    // Token::Array
    let token_array = Token::Array(vec![token_address, token_bytes, token_bool]);
    let token_array_nested = Token::Array(vec![token_string, token_array]);

    let new_array_ptr = module.asc_new(&token_array_nested);
    let new_array_obj: AscEnumArray<EthereumValueKind> =
        module.takes_ptr_returns_ptr("token_to_array", new_array_ptr);

    let new_token_ptr = module.takes_ptr_returns_ptr("token_from_array", new_array_obj);
    let new_token: Token = module.asc_get(new_token_ptr);

    assert_eq!(new_token, token_array_nested);
}

#[test]
fn abi_store_value() {
    use graph::data::store::Value;

    let valid_module = test_valid_module(mock_data_source("wasm_test/abi_store_value.wasm"));
    let mut module = WasmiModule::from_valid_module_with_ctx(valid_module, mock_context()).unwrap();

    // Value::Null
    let null_value_ptr: AscPtr<AscEnum<StoreValueKind>> = module
        .module
        .clone()
        .invoke_export("value_null", &[], &mut module)
        .expect("call failed")
        .expect("call returned nothing")
        .try_into()
        .expect("call did not return ptr");
    let null_value: Value = module.asc_get(null_value_ptr);
    assert_eq!(null_value, Value::Null);

    // Value::String
    let string = "some string";
    let string_ptr = module.asc_new(string);
    let new_value_ptr = module.takes_ptr_returns_ptr("value_from_string", string_ptr);
    let new_value: Value = module.asc_get(new_value_ptr);
    assert_eq!(new_value, Value::from(string));

    // Value::Int
    let int = i32::min_value();
    let new_value_ptr = module.takes_val_returns_ptr("value_from_int", RuntimeValue::from(int));
    let new_value: Value = module.asc_get(new_value_ptr);
    assert_eq!(new_value, Value::Int(int));

    // Value::BigDecimal
    let big_decimal = BigDecimal::from_str("3.14159001").unwrap();
    let big_decimal_ptr = module.asc_new(&big_decimal);
    let new_value_ptr = module.takes_ptr_returns_ptr("value_from_big_decimal", big_decimal_ptr);
    let new_value: Value = module.asc_get(new_value_ptr);
    assert_eq!(new_value, Value::BigDecimal(big_decimal));

    let big_decimal = BigDecimal::new(10.into(), -5);
    let big_decimal_ptr = module.asc_new(&big_decimal);
    let new_value_ptr = module.takes_ptr_returns_ptr("value_from_big_decimal", big_decimal_ptr);
    let new_value: Value = module.asc_get(new_value_ptr);
    assert_eq!(new_value, Value::BigDecimal(1_000_000.into()));

    // Value::Bool
    let boolean = true;
    let new_value_ptr = module.takes_val_returns_ptr(
        "value_from_bool",
        RuntimeValue::I32(if boolean { 1 } else { 0 }),
    );
    let new_value: Value = module.asc_get(new_value_ptr);
    assert_eq!(new_value, Value::Bool(boolean));

    // Value::List
    let new_value_ptr = module
        .module
        .clone()
        .invoke_export(
            "array_from_values",
            &[
                RuntimeValue::from(module.asc_new(string)),
                RuntimeValue::from(int),
            ],
            &mut module,
        )
        .expect("call failed")
        .expect("call returned nothing")
        .try_into()
        .expect("call did not return ptr");
    let new_value: Value = module.asc_get(new_value_ptr);
    assert_eq!(
        new_value,
        Value::List(vec![Value::from(string), Value::Int(int)])
    );

    let array: &[Value] = &[
        Value::String("foo".to_owned()),
        Value::String("bar".to_owned()),
    ];
    let array_ptr = module.asc_new(array);
    let new_value_ptr = module.takes_ptr_returns_ptr("value_from_array", array_ptr);
    let new_value: Value = module.asc_get(new_value_ptr);
    assert_eq!(
        new_value,
        Value::List(vec![
            Value::String("foo".to_owned()),
            Value::String("bar".to_owned()),
        ])
    );

    // Value::Bytes
    let bytes: &[u8] = &[0, 2, 5];
    let bytes_ptr: AscPtr<Bytes> = module.asc_new(bytes);
    let new_value_ptr = module.takes_ptr_returns_ptr("value_from_bytes", bytes_ptr);
    let new_value: Value = module.asc_get(new_value_ptr);
    assert_eq!(new_value, Value::Bytes(bytes.into()));

    // Value::BigInt
    let bytes: &[u8] = &[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1];
    let bytes_ptr: AscPtr<Uint8Array> = module.asc_new(bytes);
    let new_value_ptr = module.takes_ptr_returns_ptr("value_from_bigint", bytes_ptr);
    let new_value: Value = module.asc_get(new_value_ptr);
    assert_eq!(
        new_value,
        Value::BigInt(::graph::data::store::scalar::BigInt::from_unsigned_bytes_le(bytes))
    );
}

#[test]
fn abi_h160() {
    let valid_module = test_valid_module(mock_data_source("wasm_test/abi_classes.wasm"));
    let mut module = WasmiModule::from_valid_module_with_ctx(valid_module, mock_context()).unwrap();
    let address = H160::zero();

    // As an `Uint8Array`
    let array_buffer: AscPtr<Uint8Array> = module.asc_new(&address);
    let new_address_obj: AscPtr<Uint8Array> =
        module.takes_ptr_returns_ptr("test_address", array_buffer);

    // This should have 1 added to the first and last byte.
    let new_address: H160 = module.asc_get(new_address_obj);

    assert_eq!(
        new_address,
        H160([1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1])
    )
}

#[test]
fn string() {
    let valid_module = test_valid_module(mock_data_source("wasm_test/abi_classes.wasm"));
    let mut module = WasmiModule::from_valid_module_with_ctx(valid_module, mock_context()).unwrap();
    let string = "    漢字Double_Me🇧🇷  ";
    let trimmed_string_ptr = module.asc_new(string);
    let trimmed_string_obj: AscPtr<AscString> =
        module.takes_ptr_returns_ptr("repeat_twice", trimmed_string_ptr);
    let doubled_string: String = module.asc_get(trimmed_string_obj);
    assert_eq!(doubled_string, string.repeat(2))
}

#[test]
fn abi_big_int() {
    let valid_module = test_valid_module(mock_data_source("wasm_test/abi_classes.wasm"));
    let mut module = WasmiModule::from_valid_module_with_ctx(valid_module, mock_context()).unwrap();

    // Test passing in 0 and increment it by 1
    let old_uint = U256::zero();
    let array_buffer: AscPtr<AscBigInt> = module.asc_new(&BigInt::from_unsigned_u256(&old_uint));
    let new_uint_obj: AscPtr<AscBigInt> = module.takes_ptr_returns_ptr("test_uint", array_buffer);
    let new_uint: BigInt = module.asc_get(new_uint_obj);
    assert_eq!(new_uint, BigInt::from(1 as i32));
    let new_uint = new_uint.to_unsigned_u256();
    assert_eq!(new_uint, U256([1, 0, 0, 0]));

    // Test passing in -50 and increment it by 1
    let old_uint = BigInt::from(-50);
    let array_buffer: AscPtr<AscBigInt> = module.asc_new(&old_uint);
    let new_uint_obj: AscPtr<AscBigInt> = module.takes_ptr_returns_ptr("test_uint", array_buffer);
    let new_uint: BigInt = module.asc_get(new_uint_obj);
    assert_eq!(new_uint, BigInt::from(-49 as i32));
    let new_uint_from_u256 = BigInt::from_signed_u256(&new_uint.to_signed_u256());
    assert_eq!(new_uint, new_uint_from_u256);
}

#[test]
fn big_int_to_string() {
    let valid_module = test_valid_module(mock_data_source("wasm_test/big_int_to_string.wasm"));
    let mut module = WasmiModule::from_valid_module_with_ctx(valid_module, mock_context()).unwrap();

    let big_int_str = "30145144166666665000000000000000000";
    let big_int = BigInt::from_str(big_int_str).unwrap();
    let ptr: AscPtr<AscBigInt> = module.asc_new(&big_int);
    let string_obj: AscPtr<AscString> = module.takes_ptr_returns_ptr("big_int_to_string", ptr);
    let string: String = module.asc_get(string_obj);
    assert_eq!(string, big_int_str);
}

// This should panic rather than exhibiting UB. It's hard to test for UB, but
// when reproducing a SIGILL was observed which would be caught by this.
#[test]
#[should_panic]
fn invalid_discriminant() {
    let valid_module = test_valid_module(mock_data_source("wasm_test/abi_store_value.wasm"));
    let mut module = WasmiModule::from_valid_module_with_ctx(valid_module, mock_context()).unwrap();

    let value_ptr = module
        .module
        .clone()
        .invoke_export("invalid_discriminant", &[], &mut module)
        .expect("call failed")
        .expect("call returned nothing")
        .try_into()
        .expect("call did not return ptr");
    let _value: Value = module.asc_get(value_ptr);
}

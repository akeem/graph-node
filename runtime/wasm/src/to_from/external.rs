use ethabi;
use std::collections::HashMap;

use crate::web3::types as web3;
use graph::components::ethereum::{
    EthereumBlockData, EthereumCallData, EthereumEventData, EthereumTransactionData,
};
use graph::data::store;
use graph::prelude::serde_json;
use graph::prelude::{BigDecimal, BigInt};

use crate::asc_abi::class::*;
use crate::asc_abi::{AscHeap, AscPtr, AscType, FromAscObj, ToAscObj};

use crate::UnresolvedContractCall;

impl ToAscObj<Uint8Array> for web3::H160 {
    fn to_asc_obj<H: AscHeap>(&self, heap: &mut H) -> Uint8Array {
        self.0.to_asc_obj(heap)
    }
}

impl FromAscObj<Uint8Array> for web3::H160 {
    fn from_asc_obj<H: AscHeap>(typed_array: Uint8Array, heap: &H) -> Self {
        web3::H160(<[u8; 20]>::from_asc_obj(typed_array, heap))
    }
}

impl FromAscObj<Uint8Array> for web3::H256 {
    fn from_asc_obj<H: AscHeap>(typed_array: Uint8Array, heap: &H) -> Self {
        web3::H256(<[u8; 32]>::from_asc_obj(typed_array, heap))
    }
}

impl ToAscObj<Uint8Array> for web3::H256 {
    fn to_asc_obj<H: AscHeap>(&self, heap: &mut H) -> Uint8Array {
        self.0.to_asc_obj(heap)
    }
}

impl ToAscObj<AscBigInt> for web3::U128 {
    fn to_asc_obj<H: AscHeap>(&self, heap: &mut H) -> AscBigInt {
        let mut bytes: [u8; 16] = [0; 16];
        self.to_little_endian(&mut bytes);
        bytes.to_asc_obj(heap)
    }
}

impl ToAscObj<AscBigInt> for BigInt {
    fn to_asc_obj<H: AscHeap>(&self, heap: &mut H) -> AscBigInt {
        let bytes = self.to_signed_bytes_le();
        bytes.to_asc_obj(heap)
    }
}

impl FromAscObj<AscBigInt> for BigInt {
    fn from_asc_obj<H: AscHeap>(array_buffer: AscBigInt, heap: &H) -> Self {
        let bytes = <Vec<u8>>::from_asc_obj(array_buffer, heap);
        BigInt::from_signed_bytes_le(&bytes)
    }
}

impl ToAscObj<AscBigDecimal> for BigDecimal {
    fn to_asc_obj<H: AscHeap>(&self, heap: &mut H) -> AscBigDecimal {
        // From the docs: "Note that a positive exponent indicates a negative power of 10",
        // so "exponent" is the opposite of what you'd expect.
        let (digits, negative_exp) = self.as_bigint_and_exponent();
        AscBigDecimal {
            exp: heap.asc_new(&BigInt::from(-negative_exp)),
            digits: heap.asc_new(&BigInt::from(digits)),
        }
    }
}

impl FromAscObj<AscBigDecimal> for BigDecimal {
    fn from_asc_obj<H: AscHeap>(big_decimal: AscBigDecimal, heap: &H) -> Self {
        heap.asc_get::<BigInt, _>(big_decimal.digits)
            .to_big_decimal(heap.asc_get(big_decimal.exp))
    }
}

impl ToAscObj<AscEnum<EthereumValueKind>> for ethabi::Token {
    fn to_asc_obj<H: AscHeap>(&self, heap: &mut H) -> AscEnum<EthereumValueKind> {
        use ethabi::Token::*;

        let kind = EthereumValueKind::get_kind(self);
        let payload = match self {
            Address(address) => heap.asc_new::<AscAddress, _>(address).to_payload(),
            FixedBytes(bytes) | Bytes(bytes) => {
                heap.asc_new::<Uint8Array, _>(&**bytes).to_payload()
            }
            Int(uint) => {
                let n = BigInt::from_signed_u256(&uint);
                heap.asc_new(&n).to_payload()
            }
            Uint(uint) => {
                let n = BigInt::from_unsigned_u256(&uint);
                heap.asc_new(&n).to_payload()
            }
            Bool(b) => *b as u64,
            String(string) => heap.asc_new(&**string).to_payload(),
            FixedArray(tokens) | Array(tokens) => heap.asc_new(&**tokens).to_payload(),
            Tuple(tokens) => heap.asc_new(&**tokens).to_payload(),
        };

        AscEnum {
            kind,
            _padding: 0,
            payload: EnumPayload(payload),
        }
    }
}

impl FromAscObj<AscEnum<EthereumValueKind>> for ethabi::Token {
    fn from_asc_obj<H: AscHeap>(asc_enum: AscEnum<EthereumValueKind>, heap: &H) -> Self {
        use ethabi::Token;

        let payload = asc_enum.payload;
        match asc_enum.kind {
            EthereumValueKind::Bool => Token::Bool(bool::from(payload)),
            EthereumValueKind::Address => {
                let ptr: AscPtr<AscAddress> = AscPtr::from(payload);
                Token::Address(heap.asc_get(ptr))
            }
            EthereumValueKind::FixedBytes => {
                let ptr: AscPtr<Uint8Array> = AscPtr::from(payload);
                Token::FixedBytes(heap.asc_get(ptr))
            }
            EthereumValueKind::Bytes => {
                let ptr: AscPtr<Uint8Array> = AscPtr::from(payload);
                Token::Bytes(heap.asc_get(ptr))
            }
            EthereumValueKind::Int => {
                let ptr: AscPtr<AscBigInt> = AscPtr::from(payload);
                let n: BigInt = heap.asc_get(ptr);
                Token::Int(n.to_signed_u256())
            }
            EthereumValueKind::Uint => {
                let ptr: AscPtr<AscBigInt> = AscPtr::from(payload);
                let n: BigInt = heap.asc_get(ptr);
                Token::Uint(n.to_unsigned_u256())
            }
            EthereumValueKind::String => {
                let ptr: AscPtr<AscString> = AscPtr::from(payload);
                Token::String(heap.asc_get(ptr))
            }
            EthereumValueKind::FixedArray => {
                let ptr: AscEnumArray<EthereumValueKind> = AscPtr::from(payload);
                Token::FixedArray(heap.asc_get(ptr))
            }
            EthereumValueKind::Array => {
                let ptr: AscEnumArray<EthereumValueKind> = AscPtr::from(payload);
                Token::Array(heap.asc_get(ptr))
            }
            EthereumValueKind::Tuple => {
                let ptr: AscEnumArray<EthereumValueKind> = AscPtr::from(payload);
                Token::Tuple(heap.asc_get(ptr))
            }
        }
    }
}

impl FromAscObj<AscEnum<StoreValueKind>> for store::Value {
    fn from_asc_obj<H: AscHeap>(asc_enum: AscEnum<StoreValueKind>, heap: &H) -> Self {
        use self::store::Value;

        let payload = asc_enum.payload;
        match asc_enum.kind {
            StoreValueKind::String => {
                let ptr: AscPtr<AscString> = AscPtr::from(payload);
                Value::String(heap.asc_get(ptr))
            }
            StoreValueKind::Int => Value::Int(i32::from(payload)),
            StoreValueKind::BigDecimal => {
                let ptr: AscPtr<AscBigDecimal> = AscPtr::from(payload);
                Value::BigDecimal(heap.asc_get(ptr))
            }
            StoreValueKind::Bool => Value::Bool(bool::from(payload)),
            StoreValueKind::Array => {
                let ptr: AscEnumArray<StoreValueKind> = AscPtr::from(payload);
                Value::List(heap.asc_get(ptr))
            }
            StoreValueKind::Null => Value::Null,
            StoreValueKind::Bytes => {
                let ptr: AscPtr<Bytes> = AscPtr::from(payload);
                let array: Vec<u8> = heap.asc_get(ptr);
                Value::Bytes(array.as_slice().into())
            }
            StoreValueKind::BigInt => {
                let ptr: AscPtr<AscBigInt> = AscPtr::from(payload);
                let array: Vec<u8> = heap.asc_get(ptr);
                Value::BigInt(store::scalar::BigInt::from_signed_bytes_le(&array))
            }
        }
    }
}

impl ToAscObj<AscEnum<StoreValueKind>> for store::Value {
    fn to_asc_obj<H: AscHeap>(&self, heap: &mut H) -> AscEnum<StoreValueKind> {
        use self::store::Value;

        let payload = match self {
            Value::String(string) => heap.asc_new(string.as_str()).into(),
            Value::Int(n) => EnumPayload::from(*n),
            Value::BigDecimal(n) => heap.asc_new(n).into(),
            Value::Bool(b) => EnumPayload::from(*b),
            Value::List(array) => heap.asc_new(array.as_slice()).into(),
            Value::Null => EnumPayload(0),
            Value::Bytes(bytes) => {
                let bytes_obj: AscPtr<Uint8Array> = heap.asc_new(bytes.as_slice());
                bytes_obj.into()
            }
            Value::BigInt(big_int) => {
                let bytes_obj: AscPtr<Uint8Array> = heap.asc_new(&*big_int.to_signed_bytes_le());
                bytes_obj.into()
            }
        };

        AscEnum {
            kind: StoreValueKind::get_kind(self),
            _padding: 0,
            payload,
        }
    }
}

impl ToAscObj<AscLogParam> for ethabi::LogParam {
    fn to_asc_obj<H: AscHeap>(&self, heap: &mut H) -> AscLogParam {
        AscLogParam {
            name: heap.asc_new(self.name.as_str()),
            value: heap.asc_new(&self.value),
        }
    }
}

impl ToAscObj<AscJson> for serde_json::Map<String, serde_json::Value> {
    fn to_asc_obj<H: AscHeap>(&self, heap: &mut H) -> AscJson {
        AscTypedMap {
            entries: heap.asc_new(&*self.iter().collect::<Vec<_>>()),
        }
    }
}

impl ToAscObj<AscEntity> for HashMap<String, store::Value> {
    fn to_asc_obj<H: AscHeap>(&self, heap: &mut H) -> AscEntity {
        AscTypedMap {
            entries: heap.asc_new(&*self.iter().collect::<Vec<_>>()),
        }
    }
}

impl ToAscObj<AscEntity> for store::Entity {
    fn to_asc_obj<H: AscHeap>(&self, heap: &mut H) -> AscEntity {
        AscTypedMap {
            entries: heap.asc_new(&*self.iter().collect::<Vec<_>>()),
        }
    }
}

impl ToAscObj<AscEnum<JsonValueKind>> for serde_json::Value {
    fn to_asc_obj<H: AscHeap>(&self, heap: &mut H) -> AscEnum<JsonValueKind> {
        use serde_json::Value;

        let payload = match self {
            Value::Null => EnumPayload(0),
            Value::Bool(b) => EnumPayload::from(*b),
            Value::Number(number) => heap.asc_new(&*number.to_string()).into(),
            Value::String(string) => heap.asc_new(string.as_str()).into(),
            Value::Array(array) => heap.asc_new(array.as_slice()).into(),
            Value::Object(object) => heap.asc_new(object).into(),
        };

        AscEnum {
            kind: JsonValueKind::get_kind(self),
            _padding: 0,
            payload,
        }
    }
}

impl ToAscObj<AscEthereumBlock> for EthereumBlockData {
    fn to_asc_obj<H: AscHeap>(&self, heap: &mut H) -> AscEthereumBlock {
        AscEthereumBlock {
            hash: heap.asc_new(&self.hash),
            parent_hash: heap.asc_new(&self.parent_hash),
            uncles_hash: heap.asc_new(&self.uncles_hash),
            author: heap.asc_new(&self.author),
            state_root: heap.asc_new(&self.state_root),
            transactions_root: heap.asc_new(&self.transactions_root),
            receipts_root: heap.asc_new(&self.receipts_root),
            number: heap.asc_new(&BigInt::from(self.number)),
            gas_used: heap.asc_new(&BigInt::from_unsigned_u256(&self.gas_used)),
            gas_limit: heap.asc_new(&BigInt::from_unsigned_u256(&self.gas_limit)),
            timestamp: heap.asc_new(&BigInt::from_unsigned_u256(&self.timestamp)),
            difficulty: heap.asc_new(&BigInt::from_unsigned_u256(&self.difficulty)),
            total_difficulty: heap.asc_new(&BigInt::from_unsigned_u256(&self.total_difficulty)),
            size: self
                .size
                .map(|size| heap.asc_new(&BigInt::from_unsigned_u256(&size)))
                .unwrap_or_else(|| AscPtr::null()),
        }
    }
}

impl ToAscObj<AscEthereumTransaction> for EthereumTransactionData {
    fn to_asc_obj<H: AscHeap>(&self, heap: &mut H) -> AscEthereumTransaction {
        AscEthereumTransaction {
            hash: heap.asc_new(&self.hash),
            index: heap.asc_new(&BigInt::from(self.index)),
            from: heap.asc_new(&self.from),
            to: self
                .to
                .map(|to| heap.asc_new(&to))
                .unwrap_or_else(|| AscPtr::null()),
            value: heap.asc_new(&BigInt::from_unsigned_u256(&self.value)),
            gas_used: heap.asc_new(&BigInt::from_unsigned_u256(&self.gas_used)),
            gas_price: heap.asc_new(&BigInt::from_unsigned_u256(&self.gas_price)),
        }
    }
}

impl ToAscObj<AscEthereumTransaction_0_0_2> for EthereumTransactionData {
    fn to_asc_obj<H: AscHeap>(&self, heap: &mut H) -> AscEthereumTransaction_0_0_2 {
        AscEthereumTransaction_0_0_2 {
            hash: heap.asc_new(&self.hash),
            index: heap.asc_new(&BigInt::from(self.index)),
            from: heap.asc_new(&self.from),
            to: self
                .to
                .map(|to| heap.asc_new(&to))
                .unwrap_or_else(|| AscPtr::null()),
            value: heap.asc_new(&BigInt::from_unsigned_u256(&self.value)),
            gas_used: heap.asc_new(&BigInt::from_unsigned_u256(&self.gas_used)),
            gas_price: heap.asc_new(&BigInt::from_unsigned_u256(&self.gas_price)),
            input: heap.asc_new(&*self.input.0),
        }
    }
}

impl<T: AscType> ToAscObj<AscEthereumEvent<T>> for EthereumEventData
where
    EthereumTransactionData: ToAscObj<T>,
{
    fn to_asc_obj<H: AscHeap>(&self, heap: &mut H) -> AscEthereumEvent<T> {
        AscEthereumEvent {
            address: heap.asc_new(&self.address),
            log_index: heap.asc_new(&BigInt::from_unsigned_u256(&self.log_index)),
            transaction_log_index: heap
                .asc_new(&BigInt::from_unsigned_u256(&self.transaction_log_index)),
            log_type: self
                .log_type
                .clone()
                .map(|log_type| heap.asc_new(&log_type))
                .unwrap_or_else(|| AscPtr::null()),
            block: heap.asc_new(&self.block),
            transaction: heap.asc_new::<T, EthereumTransactionData>(&self.transaction),
            params: heap.asc_new(self.params.as_slice()),
        }
    }
}

impl ToAscObj<AscEthereumCall> for EthereumCallData {
    fn to_asc_obj<H: AscHeap>(&self, heap: &mut H) -> AscEthereumCall {
        AscEthereumCall {
            address: heap.asc_new(&self.to),
            block: heap.asc_new(&self.block),
            transaction: heap.asc_new(&self.transaction),
            inputs: heap.asc_new(self.inputs.as_slice()),
            outputs: heap.asc_new(self.outputs.as_slice()),
        }
    }
}

impl ToAscObj<AscEthereumCall_0_0_3> for EthereumCallData {
    fn to_asc_obj<H: AscHeap>(&self, heap: &mut H) -> AscEthereumCall_0_0_3 {
        AscEthereumCall_0_0_3 {
            to: heap.asc_new(&self.to),
            from: heap.asc_new(&self.from),
            block: heap.asc_new(&self.block),
            transaction: heap.asc_new(&self.transaction),
            inputs: heap.asc_new(self.inputs.as_slice()),
            outputs: heap.asc_new(self.outputs.as_slice()),
        }
    }
}

impl FromAscObj<AscUnresolvedContractCall> for UnresolvedContractCall {
    fn from_asc_obj<H: AscHeap>(asc_call: AscUnresolvedContractCall, heap: &H) -> Self {
        UnresolvedContractCall {
            contract_name: heap.asc_get(asc_call.contract_name),
            contract_address: heap.asc_get(asc_call.contract_address),
            function_name: heap.asc_get(asc_call.function_name),
            function_args: heap.asc_get(asc_call.function_args),
        }
    }
}

impl From<i32> for LogLevel {
    fn from(i: i32) -> Self {
        match i {
            0 => LogLevel::Critical,
            1 => LogLevel::Error,
            2 => LogLevel::Warning,
            3 => LogLevel::Info,
            4 => LogLevel::Debug,
            _ => LogLevel::Debug,
        }
    }
}

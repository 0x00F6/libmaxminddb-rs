#![warn(missing_docs)]
//! Derive macros for `libmaxminddb-rs` records.
//!
//! This package is normally consumed through the main crate's `derive` feature.
//! The macros generate implementations for the public traits re-exported there.

use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::ext::IdentExt;
use syn::{
    Data, DeriveInput, Expr, ExprLit, Field, Fields, GenericParam, Lit, LitByteStr, Meta, Token,
    parse_macro_input, punctuated::Punctuated,
};

fn string_value(meta: &Meta) -> syn::Result<Option<String>> {
    let Meta::NameValue(value) = meta else {
        return Ok(None);
    };
    match &value.value {
        Expr::Lit(ExprLit {
            lit: Lit::Str(value),
            ..
        }) => Ok(Some(value.value())),
        _ => Err(syn::Error::new_spanned(meta, "expected a string literal")),
    }
}

fn decode_field_key(field: &Field) -> syn::Result<String> {
    let ident = field.ident.as_ref().expect("named field");
    let mut rename = None;

    for attr in &field.attrs {
        if !attr.path().is_ident("serde") {
            continue;
        }
        let metas = attr.parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)?;
        for meta in metas {
            if !meta.path().is_ident("rename") {
                continue;
            }
            match &meta {
                Meta::NameValue(_) => rename = string_value(&meta)?,
                Meta::List(list) => {
                    let directions =
                        list.parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)?;
                    let mut serialize_name = None;
                    for direction in directions {
                        match direction
                            .path()
                            .get_ident()
                            .map(ToString::to_string)
                            .as_deref()
                        {
                            Some("deserialize") => rename = string_value(&direction)?,
                            Some("serialize") => serialize_name = string_value(&direction)?,
                            _ => {}
                        }
                    }
                    if rename.is_none() {
                        rename = serialize_name;
                    }
                }
                _ => return Err(syn::Error::new_spanned(meta, "expected serde rename value")),
            }
        }
    }

    Ok(rename.unwrap_or_else(|| ident.unraw().to_string()))
}

fn is_network_field(field: &Field) -> syn::Result<bool> {
    let mut network = false;
    for attr in &field.attrs {
        if attr.path().is_ident("mmdb") {
            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("network") {
                    network = true;
                    Ok(())
                } else {
                    Err(meta.error("unsupported mmdb attribute; expected `network`"))
                }
            })?;
        }
    }
    Ok(network)
}

#[proc_macro_derive(MmdbDecode, attributes(serde))]
/// Derives borrowed MMDB map decoding for a struct.
///
/// Fields such as `&str` and `&[u8]` borrow the source database bytes.
/// Field-level `#[serde(rename = "...")]` attributes select the corresponding MMDB map key.
/// Unsupported or missing required fields produce a decoding error.
///
/// # Examples
///
/// ```ignore
/// use libmaxminddb_rs::MmdbDecode;
/// #[derive(MmdbDecode)]
/// struct Record<'a> { country: &'a str, asn: u32 }
/// ```
pub fn derive_decode(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    derive_decode_impl(input).into()
}

fn derive_decode_impl(input: DeriveInput) -> proc_macro2::TokenStream {
    let name = input.ident;
    let Data::Struct(data) = input.data else {
        return syn::Error::new_spanned(name, "MmdbDecode only supports structs")
            .to_compile_error();
    };
    let Fields::Named(fields) = data.fields else {
        return syn::Error::new_spanned(name, "MmdbDecode requires named fields")
            .to_compile_error();
    };

    let lifetimes: Vec<_> = input
        .generics
        .params
        .iter()
        .filter_map(|p| match p {
            GenericParam::Lifetime(l) => Some(l.lifetime.clone()),
            _ => None,
        })
        .collect();
    if input
        .generics
        .params
        .iter()
        .any(|p| !matches!(p, GenericParam::Lifetime(_)))
    {
        return syn::Error::new_spanned(
            name,
            "MmdbDecode currently supports lifetime generics only",
        )
        .to_compile_error();
    }

    let decode_lt: syn::Lifetime = lifetimes
        .first()
        .cloned()
        .unwrap_or_else(|| syn::parse_quote!('__mmdb));
    let impl_generics = if lifetimes.is_empty() {
        quote!(<'__mmdb>)
    } else {
        let ls = &lifetimes;
        quote!(<#(#ls),*>)
    };
    let ty_generics = if lifetimes.is_empty() {
        quote!()
    } else {
        let ls = &lifetimes;
        quote!(<#(#ls),*>)
    };

    // Decode generated structs with one linear map pass instead of calling
    // `ValueRef::get` once per field (O(fields * entries)).  Each slot only accepts
    // its first match, preserving the previous first-duplicate-wins behaviour.
    let field_count = fields.named.len();
    let field_info = fields
        .named
        .iter()
        .enumerate()
        .map(|(index, f)| {
            let ident = f.ident.as_ref().expect("named field").clone();
            // `r#type` must match the MMDB key `type`, not `r#type`.
            let key = decode_field_key(f)?;
            // Use an index because renamed MMDB keys may contain punctuation.
            let slot = format_ident!("__mmdb_field_{index}");
            let ty = f.ty.clone();
            Ok((ident, slot, key, ty))
        })
        .collect::<syn::Result<Vec<_>>>();
    let field_info = match field_info {
        Ok(field_info) => field_info,
        Err(error) => return error.to_compile_error(),
    };
    let declarations = field_info.iter().map(|(_, slot, _, _)| {
        quote! { let mut #slot = None; }
    });
    let match_arms = field_info.iter().map(|(_, slot, key, _)| {
        quote! {
            #key if #slot.is_none() => {
                #slot = Some(__mmdb_value);
                __mmdb_matched += 1;
            }
        }
    });
    let initializers = field_info.iter().map(|(ident, slot, _, ty)| {
        quote! {
            #ident: <#ty as ::libmaxminddb_rs::DecodeField<#decode_lt>>::decode_field(#slot)?
        }
    });

    // Single-pass decoder over the encoded map: each key is compared as raw
    // bytes with the field names, matching values are decoded in place, and
    // unknown or duplicate entries are skipped without being decoded. The loop
    // stops as soon as every field is filled; `finish_map` then skips the tail
    // only when the parent still has to read past it.
    let raw_declarations = field_info.iter().map(|(_, slot, _, ty)| {
        quote! { let mut #slot: ::core::option::Option<#ty> = ::core::option::Option::None; }
    });
    let raw_arms = field_info.iter().map(|(_, slot, key, ty)| {
        let key_bytes = LitByteStr::new(key.as_bytes(), proc_macro2::Span::call_site());
        quote! {
            #key_bytes if #slot.is_none() => {
                #slot = ::core::option::Option::Some(
                    <#ty as ::libmaxminddb_rs::DecodeField<#decode_lt>>::decode_raw(__mmdb_decoder)?,
                );
                __mmdb_matched += 1;
                if __mmdb_matched == #field_count {
                    break;
                }
            }
        }
    });
    let raw_initializers = field_info.iter().map(|(ident, slot, _, ty)| {
        quote! {
            #ident: match #slot {
                ::core::option::Option::Some(value) => value,
                ::core::option::Option::None => {
                    <#ty as ::libmaxminddb_rs::DecodeField<#decode_lt>>::decode_missing()?
                }
            }
        }
    });

    quote! {
        impl #impl_generics ::libmaxminddb_rs::MmdbDecode<#decode_lt> for #name #ty_generics {
            fn decode(value: &::libmaxminddb_rs::ValueRef<#decode_lt>) -> ::libmaxminddb_rs::Result<Self> {
                let __mmdb_entries = match value {
                    ::libmaxminddb_rs::ValueRef::Map(entries) => entries,
                    _ => return Err(::libmaxminddb_rs::Error::DecodingError("MmdbDecode expected a map".into())),
                };
                #(#declarations)*
                let mut __mmdb_matched = 0usize;
                for (__mmdb_key, __mmdb_value) in __mmdb_entries {
                    match *__mmdb_key {
                        #(#match_arms,)*
                        _ => {}
                    }
                    if __mmdb_matched == #field_count {
                        break;
                    }
                }
                Ok(Self { #(#initializers),* })
            }

            #[inline]
            #[allow(unused_mut, unused_variables)]
            fn decode_raw(
                __mmdb_decoder: &mut ::libmaxminddb_rs::__private::RawDecoder<#decode_lt>,
            ) -> ::libmaxminddb_rs::Result<Self> {
                let __mmdb_map = __mmdb_decoder.enter_map("MmdbDecode expected a map")?;
                #(#raw_declarations)*
                let mut __mmdb_matched = 0usize;
                let mut __mmdb_remaining = __mmdb_map.len();
                while __mmdb_remaining != 0 {
                    __mmdb_remaining -= 1;
                    match __mmdb_decoder.read_key()? {
                        #(#raw_arms)*
                        _ => __mmdb_decoder.skip_value()?,
                    }
                }
                __mmdb_decoder.finish_map(__mmdb_map, __mmdb_remaining)?;
                Ok(Self { #(#raw_initializers),* })
            }
        }

        impl #impl_generics ::libmaxminddb_rs::DecodeField<#decode_lt> for #name #ty_generics {
            fn decode_field(
                value: Option<&::libmaxminddb_rs::ValueRef<#decode_lt>>,
            ) -> ::libmaxminddb_rs::Result<Self> {
                let value = value.ok_or_else(|| {
                    ::libmaxminddb_rs::Error::DecodingError("missing nested MMDB struct".into())
                })?;
                <Self as ::libmaxminddb_rs::MmdbDecode<#decode_lt>>::decode(value)
            }

            #[inline]
            fn decode_raw(
                decoder: &mut ::libmaxminddb_rs::__private::RawDecoder<#decode_lt>,
            ) -> ::libmaxminddb_rs::Result<Self> {
                <Self as ::libmaxminddb_rs::MmdbDecode<#decode_lt>>::decode_raw(decoder)
            }
        }
    }
}

#[proc_macro_derive(MmdbEncode, attributes(mmdb))]
/// Derives encoding of struct fields to an owned MMDB value.
///
/// `Option::None` fields are omitted because MMDB has no null data type.
///
/// # Examples
///
/// ```ignore
/// use libmaxminddb_rs::MmdbEncode;
/// #[derive(MmdbEncode)]
/// struct Record<'a> { country: &'a str, asn: u32 }
/// ```
pub fn derive_encode(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    derive_encode_impl(input).into()
}

fn derive_encode_impl(input: DeriveInput) -> proc_macro2::TokenStream {
    let name = input.ident;
    let generics = input.generics;
    let Data::Struct(data) = input.data else {
        return syn::Error::new_spanned(name, "MmdbEncode only supports structs")
            .to_compile_error();
    };
    let Fields::Named(fields) = data.fields else {
        return syn::Error::new_spanned(name, "MmdbEncode requires named fields")
            .to_compile_error();
    };

    let mut inserts = Vec::new();
    for field in &fields.named {
        match is_network_field(field) {
            Ok(true) => continue,
            Ok(false) => {}
            Err(error) => return error.to_compile_error(),
        }
        let ident = field.ident.as_ref().expect("named field");
        let key = ident.to_string();
        inserts.push(quote! {
            if let Some(value) = ::libmaxminddb_rs::EncodeField::encode_optional_field(&self.#ident)? {
                map.insert(#key.to_owned(), value);
            }
        });
    }
    let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();
    quote! {
        impl #impl_generics ::libmaxminddb_rs::MmdbEncode for #name #ty_generics #where_clause {
            fn encode(&self) -> ::libmaxminddb_rs::Result<::libmaxminddb_rs::Value> {
                let mut map = ::std::collections::BTreeMap::new();
                #(#inserts)*
                Ok(::libmaxminddb_rs::Value::Map(map))
            }
        }

        impl #impl_generics ::libmaxminddb_rs::EncodeField for #name #ty_generics #where_clause {
            fn encode_field(&self) -> ::libmaxminddb_rs::Result<::libmaxminddb_rs::Value> {
                <Self as ::libmaxminddb_rs::MmdbEncode>::encode(self)
            }
        }
    }
}

#[proc_macro_derive(MmdbRecord, attributes(mmdb))]
/// Derives the network key for one-object writer insertion.
///
/// Mark exactly one `IpNetwork` field with `#[mmdb(network)]`; that field is
/// excluded from the encoded payload when paired with `MmdbEncode`.
///
/// # Examples
///
/// ```ignore
/// use libmaxminddb_rs::{IpNetwork, MmdbEncode, MmdbRecord};
/// #[derive(MmdbEncode, MmdbRecord)]
/// struct Record { #[mmdb(network)] network: IpNetwork, asn: u32 }
/// ```
pub fn derive_record(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    derive_record_impl(input).into()
}

fn derive_record_impl(input: DeriveInput) -> proc_macro2::TokenStream {
    let name = input.ident;
    let generics = input.generics;
    let Data::Struct(data) = input.data else {
        return syn::Error::new_spanned(name, "MmdbRecord only supports structs")
            .to_compile_error();
    };
    let Fields::Named(fields) = data.fields else {
        return syn::Error::new_spanned(name, "MmdbRecord requires named fields")
            .to_compile_error();
    };

    let mut network_field = None;
    for field in &fields.named {
        match is_network_field(field) {
            Ok(true) if network_field.is_none() => network_field = field.ident.clone(),
            Ok(true) => {
                return syn::Error::new_spanned(
                    field,
                    "MmdbRecord requires exactly one #[mmdb(network)] field",
                )
                .to_compile_error();
            }
            Ok(false) => {}
            Err(error) => return error.to_compile_error(),
        }
    }
    let Some(network_field) = network_field else {
        return syn::Error::new_spanned(name, "MmdbRecord requires one #[mmdb(network)] field")
            .to_compile_error();
    };
    let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();
    quote! {
        impl #impl_generics ::libmaxminddb_rs::MmdbRecord for #name #ty_generics #where_clause {
            fn network(&self) -> ::libmaxminddb_rs::IpNetwork {
                ::core::clone::Clone::clone(&self.#network_field)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use syn::parse_quote;

    /// The `syn::parse_quote!` inputs below are plain `DeriveInput`s, so the
    /// expansion helpers run directly in the test harness without needing a
    /// `proc_macro::TokenStream` (which cannot be constructed off the
    /// compiler's proc-macro bridge).
    fn tokens(input: DeriveInput) -> String {
        derive_decode_impl(input).to_string()
    }

    fn tokens_encode(input: DeriveInput) -> String {
        derive_encode_impl(input).to_string()
    }

    fn tokens_record(input: DeriveInput) -> String {
        derive_record_impl(input).to_string()
    }

    #[test]
    fn decode_plain_struct_without_generics() {
        let out = tokens(parse_quote! {
            struct Plain {
                a: u32,
                b: String,
            }
        });
        assert!(
            out.contains("impl < '__mmdb > :: libmaxminddb_rs :: MmdbDecode < '__mmdb > for Plain")
        );
        assert!(
            out.contains(
                "impl < '__mmdb > :: libmaxminddb_rs :: DecodeField < '__mmdb > for Plain"
            )
        );
        assert!(out.contains(":: libmaxminddb_rs :: ValueRef < '__mmdb >"));
        assert!(!out.contains("compile_error"));
    }

    #[test]
    fn decode_struct_with_multiple_lifetimes() {
        let out = tokens(parse_quote! {
            struct Multi<'a, 'b> {
                first: &'a str,
                second: &'b str,
            }
        });
        assert!(out.contains(
            "impl < 'a , 'b > :: libmaxminddb_rs :: MmdbDecode < 'a > for Multi < 'a , 'b >"
        ));
        assert!(!out.contains("compile_error"));
    }

    #[test]
    fn decode_enum_is_rejected() {
        let out = tokens(parse_quote! {
            enum Wrong {}
        });
        assert!(out.contains("MmdbDecode only supports structs"));
        assert!(out.contains("compile_error"));
    }

    #[test]
    fn decode_unamed_fields_are_rejected() {
        let out = tokens(parse_quote! {
            struct Tuple(u32);
        });
        assert!(out.contains("MmdbDecode requires named fields"));
    }

    #[test]
    fn decode_type_generics_are_rejected() {
        let out = tokens(parse_quote! {
            struct Generic<T> {
                x: T,
            }
        });
        assert!(out.contains("MmdbDecode currently supports lifetime generics only"));
    }

    #[test]
    fn encode_plain_struct() {
        let out = tokens_encode(parse_quote! {
            struct Out<'a> {
                a: &'a str,
                b: Option<u32>,
            }
        });
        assert!(out.contains("impl < 'a > :: libmaxminddb_rs :: MmdbEncode for Out < 'a >"));
        assert!(out.contains("impl < 'a > :: libmaxminddb_rs :: EncodeField for Out < 'a >"));
        assert!(out.contains("BTreeMap"));
        assert!(!out.contains("compile_error"));
    }

    #[test]
    fn encode_skips_network_field() {
        let out = tokens_encode(parse_quote! {
            struct Entry {
                #[mmdb(network)]
                network: String,
                payload: u32,
            }
        });
        assert!(!out.contains("network"));
        assert!(out.contains("payload"));
        assert!(!out.contains("compile_error"));
    }

    #[test]
    fn encode_enum_is_rejected() {
        let out = tokens_encode(parse_quote! {
            enum Wrong {}
        });
        assert!(out.contains("MmdbEncode only supports structs"));
    }

    #[test]
    fn encode_unamed_fields_are_rejected() {
        let out = tokens_encode(parse_quote! {
            struct Tuple(u32);
        });
        assert!(out.contains("MmdbEncode requires named fields"));
    }

    #[test]
    fn encode_unknown_mmdb_attribute_is_rejected() {
        let out = tokens_encode(parse_quote! {
            struct Bad {
                #[mmdb(other)]
                a: u32,
            }
        });
        assert!(out.contains("unsupported mmdb attribute"));
    }

    #[test]
    fn record_plain_struct() {
        let out = tokens_record(parse_quote! {
            struct R {
                #[mmdb(network)]
                network: String,
                a: u32,
            }
        });
        assert!(out.contains("impl :: libmaxminddb_rs :: MmdbRecord for R"));
        assert!(out.contains(". network"));
        assert!(!out.contains("compile_error"));
    }

    #[test]
    fn record_enum_is_rejected() {
        let out = tokens_record(parse_quote! {
            enum Wrong {}
        });
        assert!(out.contains("MmdbRecord only supports structs"));
    }

    #[test]
    fn record_unamed_fields_are_rejected() {
        let out = tokens_record(parse_quote! {
            struct Tuple(u32);
        });
        assert!(out.contains("MmdbRecord requires named fields"));
    }

    #[test]
    fn record_requires_exactly_one_network_field() {
        let out = tokens_record(parse_quote! {
            struct Bad {
                #[mmdb(network)]
                network: String,
                #[mmdb(network)]
                also_network: String,
            }
        });
        assert!(out.contains("MmdbRecord requires exactly one #[mmdb(network)] field"));
    }

    #[test]
    fn record_requires_a_network_field() {
        let out = tokens_record(parse_quote! {
            struct Bad {
                a: u32,
            }
        });
        assert!(out.contains("MmdbRecord requires one #[mmdb(network)] field"));
    }

    #[test]
    fn record_unknown_mmdb_attribute_is_rejected() {
        let out = tokens_record(parse_quote! {
            struct Bad {
                #[mmdb(other)]
                a: u32,
            }
        });
        assert!(out.contains("unsupported mmdb attribute"));
    }

    #[test]
    fn malformed_network_attribute_is_rejected() {
        let input: DeriveInput = parse_quote! {
            struct Bad {
                #[mmdb(network = true)]
                network: String,
            }
        };
        let out = tokens_record(input.clone());
        assert!(out.contains("compile_error"));
        assert!(!out.contains("impl :: libmaxminddb_rs :: MmdbRecord"));
        assert!(tokens_encode(input).contains("compile_error"));
    }
}

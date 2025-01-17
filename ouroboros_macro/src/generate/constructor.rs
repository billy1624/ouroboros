use crate::{
    info_structures::{ArgType, FieldType, Options, StructInfo},
    utils::to_class_case,
};
use proc_macro2::{Ident, TokenStream};
use quote::{format_ident, quote};
use syn::Error;

pub fn create_builder_and_constructor(
    info: &StructInfo,
    options: Options,
    make_async: bool,
) -> Result<(Ident, TokenStream, TokenStream), Error> {
    let struct_name = info.ident.clone();
    let generic_args = info.generic_arguments();

    let vis = if options.do_pub_extras {
        info.vis.clone()
    } else {
        syn::parse_quote! { pub(super) }
    };
    let builder_struct_name = if make_async {
        format_ident!("{}AsyncBuilder", info.ident)
    } else {
        format_ident!("{}Builder", info.ident)
    };
    let documentation = format!(
        concat!(
            "Constructs a new instance of this self-referential struct. (See also ",
            "[`{0}::build()`]({0}::build)). Each argument is a field of ",
            "the new struct. Fields that refer to other fields inside the struct are initialized ",
            "using functions instead of directly passing their value. The arguments are as ",
            "follows:\n\n| Argument | Suggested Use |\n| --- | --- |\n",
        ),
        builder_struct_name.to_string()
    );
    let builder_documentation = concat!(
        "A more verbose but stable way to construct self-referencing structs. It is ",
        "comparable to using `StructName { field1: value1, field2: value2 }` rather than ",
        "`StructName::new(value1, value2)`. This has the dual benefit of making your code ",
        "both easier to refactor and more readable. Call [`build()`](Self::build) to ",
        "construct the actual struct. The fields of this struct should be used as follows:\n\n",
        "| Field | Suggested Use |\n| --- | --- |\n",
    )
    .to_owned();
    let build_fn_documentation = format!(
        concat!(
            "Calls [`{0}::new()`]({0}::new) using the provided values. This is preferrable over ",
            "calling `new()` directly for the reasons listed above. "
        ),
        info.ident.to_string()
    );
    let mut doc_table = "".to_owned();
    let mut code: Vec<TokenStream> = Vec::new();
    let mut params: Vec<TokenStream> = Vec::new();
    let mut builder_struct_generic_producers: Vec<_> = info
        .generic_params()
        .iter()
        .map(|param| quote! { #param })
        .collect();
    let mut builder_struct_generic_consumers = info.generic_arguments();
    let mut builder_struct_fields = Vec::new();
    let mut builder_struct_field_names = Vec::new();

    // code.push(quote! { let mut result = ::core::mem::MaybeUninit::<Self>::uninit(); });

    for field in &info.fields {
        let field_name = &field.name;

        let arg_type = field.make_constructor_arg_type(&info, make_async)?;
        if let ArgType::Plain(plain_type) = arg_type {
            // No fancy builder function, we can just move the value directly into the struct.
            params.push(quote! { #field_name: #plain_type });
            builder_struct_fields.push(quote! { #field_name: #plain_type });
            builder_struct_field_names.push(quote! { #field_name });
            doc_table += &format!(
                "| `{}` | Directly pass in the value this field should contain |\n",
                field_name.to_string()
            );
        } else if let ArgType::TraitBound(bound_type) = arg_type {
            // Trait bounds are much trickier. We need a special syntax to accept them in the
            // contructor, and generic parameters need to be added to the builder struct to make
            // it work.
            let builder_name = field.builder_name();
            params.push(quote! { #builder_name : impl #bound_type });
            doc_table += &format!(
                "| `{}` | Use a function or closure: `(",
                builder_name.to_string()
            );
            let mut builder_args = Vec::new();
            for (index, borrow) in field.borrows.iter().enumerate() {
                let borrowed_name = &info.fields[borrow.index].name;
                builder_args.push(format_ident!("{}_illegal_static_reference", borrowed_name));
                doc_table += &format!(
                    "{}: &{}_",
                    borrowed_name.to_string(),
                    if borrow.mutable { "mut " } else { "" },
                );
                if index < field.borrows.len() - 1 {
                    doc_table += ", ";
                }
            }
            doc_table += &format!(") -> {}: _` | \n", field_name.to_string());
            if make_async {
                code.push(quote! { let #field_name = #builder_name (#(#builder_args),*).await; });
            } else {
                code.push(quote! { let #field_name = #builder_name (#(#builder_args),*); });
            }
            let generic_type_name =
                format_ident!("{}Builder_", to_class_case(field_name.to_string().as_str()));

            builder_struct_generic_producers.push(quote! { #generic_type_name: #bound_type });
            builder_struct_generic_consumers.push(quote! { #generic_type_name });
            builder_struct_fields.push(quote! { #builder_name: #generic_type_name });
            builder_struct_field_names.push(quote! { #builder_name });
        }
        if field.is_borrowed() {
            let boxed = field.boxed();
            if field.field_type == FieldType::BorrowedMut {
                code.push(quote! { let mut #field_name = #boxed; });
            } else {
                code.push(quote! { let #field_name = #boxed; });
            }
        };

        if field.field_type == FieldType::Borrowed {
            code.push(field.make_illegal_static_reference());
        } else if field.field_type == FieldType::BorrowedMut {
            code.push(field.make_illegal_static_mut_reference());
        }
    }

    let documentation = if !options.do_no_doc {
        let documentation = documentation + &doc_table;
        quote! {
            #[doc=#documentation]
        }
    } else {
        quote! { #[doc(hidden)] }
    };

    let builder_documentation = if !options.do_no_doc {
        let builder_documentation = builder_documentation + &doc_table;
        quote! {
            #[doc=#builder_documentation]
        }
    } else {
        quote! { #[doc(hidden)] }
    };

    let constructor_fn = if make_async {
        quote! { async fn new_async }
    } else {
        quote! { fn new }
    };
    let field_names: Vec<_> = info.fields.iter().map(|field| field.name.clone()).collect();
    let constructor_def = quote! {
        #documentation
        #vis #constructor_fn(#(#params),*) -> #struct_name <#(#generic_args),*> {
            #(#code)*
            Self {
                #(#field_names),*
            }
        }
    };
    let generic_where = &info.generics.where_clause;
    let builder_fn = if make_async {
        quote! { async fn build }
    } else {
        quote! { fn build }
    };
    let builder_code = if make_async {
        quote! {
            #struct_name::new_async(
                #(self.#builder_struct_field_names),*
            ).await
        }
    } else {
        quote! {
            #struct_name::new(
                #(self.#builder_struct_field_names),*
            )
        }
    };
    let builder_def = quote! {
        #builder_documentation
        #vis struct #builder_struct_name <#(#builder_struct_generic_producers),*> #generic_where {
            #(#vis #builder_struct_fields),*
        }
        impl<#(#builder_struct_generic_producers),*> #builder_struct_name <#(#builder_struct_generic_consumers),*> #generic_where {
            #[doc=#build_fn_documentation]
            #vis #builder_fn(self) -> #struct_name <#(#generic_args),*> {
                #builder_code
            }
        }
    };
    Ok((builder_struct_name, builder_def, constructor_def))
}

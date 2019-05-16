use std::{mem, rc::Rc};

use log::{debug, info};
use petgraph::graph::NodeIndex;
use rustc_hash::FxHashMap;

use crate::{error::Result, typemap::TypeMap};

impl TypeMap {
    pub(crate) fn merge(
        &mut self,
        id_of_code: &str,
        code: &str,
        target_pointer_width: usize,
    ) -> Result<()> {
        debug!("merging {} with our rules", id_of_code);
        let mut was_traits_usage_code = FxHashMap::default();
        mem::swap(&mut was_traits_usage_code, &mut self.traits_usage_code);
        let mut new_data = crate::typemap::parse::parse(
            id_of_code,
            code,
            target_pointer_width,
            was_traits_usage_code,
        )?;
        mem::swap(&mut new_data.traits_usage_code, &mut self.traits_usage_code);

        let mut new_node_to_our_map = FxHashMap::<NodeIndex, NodeIndex>::default();
        add_new_nodes(&new_data, self, &mut new_node_to_our_map);
        add_new_edges(&new_data, self, &new_node_to_our_map);
        add_new_ftypes(&new_data, self, &new_node_to_our_map);

        self.utils_code.append(&mut new_data.utils_code);
        //TODO: more intellect to process new generics
        self.generic_edges.append(&mut new_data.generic_edges);
        Ok(())
    }
}

fn add_new_nodes(
    new_data: &TypeMap,
    data: &mut TypeMap,
    new_node_to_our_map: &mut FxHashMap<NodeIndex, NodeIndex>,
) {
    for new_node_idx in new_data.conv_graph.node_indices() {
        let new_node = &new_data.conv_graph[new_node_idx];
        let data_rust_names_map = &mut data.rust_names_map;
        let data_conv_graph = &mut data.conv_graph;
        let data_idx = *data_rust_names_map
            .entry(new_node.normalized_name.clone())
            .or_insert_with(|| {
                let idx = data_conv_graph.add_node((*new_node).clone());
                Rc::make_mut(&mut data_conv_graph[idx]).graph_idx = idx;
                idx
            });
        Rc::make_mut(&mut data_conv_graph[data_idx]).merge(&new_data.conv_graph[new_node_idx]);
        new_node_to_our_map.insert(new_node_idx, data_idx);
    }
}

fn add_new_edges(
    new_data: &TypeMap,
    data: &mut TypeMap,
    new_node_to_our_map: &FxHashMap<NodeIndex, NodeIndex>,
) {
    for (new_node_idx, our_idx) in new_node_to_our_map {
        let mut new_edges = new_data
            .conv_graph
            .neighbors_directed(*new_node_idx, petgraph::Outgoing)
            .detach();
        while let Some((new_edge, new_target)) = new_edges.next(&new_data.conv_graph) {
            let our_target = *new_node_to_our_map
                .get(&new_target)
                .expect("At this step we should have full map new -> our");
            if let Some(existing_edge) = data.conv_graph.find_edge(*our_idx, our_target) {
                info!(
                    "typemap merge: replace {:?} with new conversation rule {:?}, for {} -> {}",
                    data.conv_graph[existing_edge],
                    new_data.conv_graph[new_edge],
                    data.conv_graph[*our_idx],
                    data.conv_graph[our_target],
                );
            }
            data.conv_graph.update_edge(
                *our_idx,
                our_target,
                new_data.conv_graph[new_edge].clone(),
            );
        }
    }
}

fn add_new_ftypes(
    new_data: &TypeMap,
    data: &mut TypeMap,
    new_node_to_our_map: &FxHashMap<NodeIndex, NodeIndex>,
) {
    for (new_ftype, correspoding_rtype) in &new_data.foreign_names_map {
        let our_rtype = new_node_to_our_map
            .get(correspoding_rtype)
            .expect("At this step we should have full map new -> our");
        *data
            .foreign_names_map
            .entry(new_ftype.clone())
            .or_insert(*our_rtype) = *our_rtype;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::typemap::find_conversation_path;
    use proc_macro2::Span;
    use rustc_hash::FxHashSet;
    use syn::{parse_quote, Type};

    #[test]
    fn test_merge() {
        let mut types_map = TypeMap::default();
        types_map
            .merge(
                "base",
                r#"
mod swig_foreign_types_map {
    #![swig_foreigner_type="boolean"]
    #![swig_rust_type="jboolean"]
    #![swig_foreigner_type="int"]
    #![swig_rust_type="jint"]
}
"#,
                64,
            )
            .unwrap();
        types_map
            .merge(
                "test_merge",
                r#"
mod swig_foreign_types_map {
    #![swig_foreigner_type="boolean"]
    #![swig_rust_type="jboolean"]
    #![swig_foreigner_type="int"]
    #![swig_rust_type="jint"]
}

fn helper1() {
}

#[swig_code = "let mut {to_var}: {to_var_type} = {from_var}.swig_into(env);"]
trait SwigInto<T> {
    fn swig_into(self, env: *mut JNIEnv) -> T;
}

#[swig_code = "let mut {to_var}: {to_var_type} = <{to_var_type}>::swig_from({from_var}, env);"]
trait SwigFrom<T> {
    fn swig_from(T, env: *mut JNIEnv) -> Self;
}

impl SwigInto<bool> for jboolean {
    fn swig_into(self, _: *mut JNIEnv) -> bool {
        self != 0
    }
}

fn helper2() {
}

impl SwigFrom<bool> for jboolean {
    fn swig_from(x: bool, _: *mut JNIEnv) -> Self {
        if x { 1 as jboolean } else { 0 as jboolean }
    }
}
impl SwigFrom<i32> for jint {
    fn swig_from(x: i32, _: *mut JNIEnv) -> Self {
        x
    }
}

fn helper3() {
}
"#,
                64,
            )
            .unwrap();
        assert_eq!(
            {
                let mut set = FxHashSet::default();
                for k in types_map.foreign_names_map.keys() {
                    set.insert(k.clone());
                }
                set
            },
            {
                let mut set = FxHashSet::default();
                set.insert("boolean".into());
                set.insert("int".into());
                set
            }
        );
        let ty_i32 = types_map.find_or_alloc_rust_type(&parse_type! { i32 });
        assert_eq!(
            types_map
                .map_through_conversation_to_foreign(
                    &ty_i32,
                    petgraph::Direction::Outgoing,
                    Span::call_site(),
                    |_, fc| fc.constructor_ret_type.clone(),
                )
                .unwrap()
                .name,
            "int"
        );
        assert_eq!(
            "let mut {to_var}: {to_var_type} = {from_var}.swig_into(env);",
            {
                let from = types_map.rust_names_map["jboolean"];
                let to = types_map.rust_names_map["bool"];
                let conv = &types_map.conv_graph[types_map.conv_graph.find_edge(from, to).unwrap()];
                conv.code_template.clone()
            },
        );

        let from = types_map.rust_names_map["jboolean"];
        let to = types_map.rust_names_map["bool"];
        assert_eq!(
            find_conversation_path(&types_map.conv_graph, from, to, Span::call_site()).unwrap(),
            vec![types_map.conv_graph.find_edge(from, to).unwrap()]
        );

        let from = types_map.rust_names_map["bool"];
        let to = types_map.rust_names_map["jboolean"];
        assert_eq!(
            find_conversation_path(&types_map.conv_graph, from, to, Span::call_site()).unwrap(),
            vec![types_map.conv_graph.find_edge(from, to).unwrap()]
        );
        assert_eq!(
            types_map
                .utils_code
                .iter()
                .filter_map(|v| {
                    let item: syn::Item = v.clone();
                    match item {
                        syn::Item::Fn(ref fun) => Some(fun.ident.to_string()),
                        syn::Item::Trait(ref trait_) => Some(trait_.ident.to_string()),
                        _ => None,
                    }
                })
                .collect::<Vec<_>>(),
            vec!["helper1", "SwigInto", "SwigFrom", "helper2", "helper3"]
        );
    }
}

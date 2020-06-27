use roc_collections::all::{relative_complement, union, MutMap, SendSet};
use roc_module::ident::{Lowercase, TagName};
use roc_module::symbol::Symbol;
use roc_types::boolean_algebra::Bool;
use roc_types::subs::Content::{self, *};
use roc_types::subs::{Descriptor, FlatType, Mark, OptVariable, Subs, Variable};
use roc_types::types::{gather_fields, ErrorType, Mismatch, RecordStructure};
use std::hash::Hash;

macro_rules! mismatch {
    () => {{
        if cfg!(debug_assertions) {
            println!(
                "Mismatch in {} Line {} Column {}",
                file!(),
                line!(),
                column!()
            );
        }

        vec![Mismatch::TypeMismatch]
    }};
    ($msg:expr) => {{
        if cfg!(debug_assertions) {
            println!(
                "Mismatch in {} Line {} Column {}",
                file!(),
                line!(),
                column!()
            );
            println!($msg);
            println!("");
        }

        vec![Mismatch::TypeMismatch]
    }};
    ($msg:expr,) => {{
        if cfg!(debug_assertions) {
            println!(
                "Mismatch in {} Line {} Column {}",
                file!(),
                line!(),
                column!()
            );
            println!($msg);
            println!("");
        }

        vec![Mismatch::TypeMismatch]
    }};
    ($msg:expr, $($arg:tt)*) => {{
        if cfg!(debug_assertions) {
            println!(
                "Mismatch in {} Line {} Column {}",
                file!(),
                line!(),
                column!()
            );
            println!($msg, $($arg)*);
            println!("");
        }

        vec![Mismatch::TypeMismatch]
    }};
}

type Pool = Vec<Variable>;

pub struct Context {
    pub first: Variable,
    pub first_desc: Descriptor,
    pub second: Variable,
    pub second_desc: Descriptor,
}

#[derive(Debug)]
pub enum Unified {
    Success(Pool),
    Failure(Pool, ErrorType, ErrorType),
    BadType(Pool, roc_types::types::Problem),
}

#[derive(Debug)]
struct TagUnionStructure {
    tags: MutMap<TagName, Vec<Variable>>,
    ext: Variable,
}

type Outcome = Vec<Mismatch>;

#[inline(always)]
pub fn unify(subs: &mut Subs, var1: Variable, var2: Variable) -> Unified {
    let mut vars = Vec::new();
    let mismatches = unify_pool(subs, &mut vars, var1, var2);

    if mismatches.is_empty() {
        Unified::Success(vars)
    } else {
        let (type1, mut problems) = subs.var_to_error_type(var1);
        let (type2, problems2) = subs.var_to_error_type(var2);

        problems.extend(problems2);

        subs.union(var1, var2, Content::Error.into());

        if !problems.is_empty() {
            Unified::BadType(vars, problems.remove(0))
        } else {
            Unified::Failure(vars, type1, type2)
        }
    }
}

#[inline(always)]
pub fn unify_pool(subs: &mut Subs, pool: &mut Pool, var1: Variable, var2: Variable) -> Outcome {
    if subs.equivalent(var1, var2) {
        Vec::new()
    } else {
        let ctx = Context {
            first: var1,
            first_desc: subs.get(var1),
            second: var2,
            second_desc: subs.get(var2),
        };

        unify_context(subs, pool, ctx)
    }
}

fn unify_context(subs: &mut Subs, pool: &mut Pool, ctx: Context) -> Outcome {
    let print_debug_messages = false;
    if print_debug_messages {
        let (type1, _problems1) = subs.var_to_error_type(ctx.first);
        let (type2, _problems2) = subs.var_to_error_type(ctx.second);
        println!("\n --------------- \n");
        dbg!(ctx.first, type1);
        println!("\n --- \n");
        dbg!(ctx.second, type2);
        println!("\n --------------- \n");
    }
    match &ctx.first_desc.content {
        FlexVar(opt_name) => unify_flex(subs, pool, &ctx, opt_name, &ctx.second_desc.content),
        RigidVar(name) => unify_rigid(subs, &ctx, name, &ctx.second_desc.content),
        Structure(flat_type) => {
            unify_structure(subs, pool, &ctx, flat_type, &ctx.second_desc.content)
        }
        Alias(symbol, args, real_var) => unify_alias(subs, pool, &ctx, *symbol, args, *real_var),
        Error => {
            // Error propagates. Whatever we're comparing it to doesn't matter!
            merge(subs, &ctx, Error)
        }
    }
}

#[inline(always)]
fn unify_alias(
    subs: &mut Subs,
    pool: &mut Pool,
    ctx: &Context,
    symbol: Symbol,
    args: &[(Lowercase, Variable)],
    real_var: Variable,
) -> Outcome {
    let other_content = &ctx.second_desc.content;

    match other_content {
        FlexVar(_) => {
            // Alias wins
            merge(subs, &ctx, Alias(symbol, args.to_owned(), real_var))
        }
        RigidVar(_) => unify_pool(subs, pool, real_var, ctx.second),
        Alias(other_symbol, other_args, other_real_var) => {
            if symbol == *other_symbol {
                if args.len() == other_args.len() {
                    let mut problems = Vec::new();
                    for ((_, l_var), (_, r_var)) in args.iter().zip(other_args.iter()) {
                        problems.extend(unify_pool(subs, pool, *l_var, *r_var));
                    }

                    problems.extend(merge(subs, &ctx, other_content.clone()));

                    problems
                } else {
                    mismatch!()
                }
            } else {
                unify_pool(subs, pool, real_var, *other_real_var)
            }
        }
        Structure(_) => unify_pool(subs, pool, real_var, ctx.second),
        Error => merge(subs, ctx, Error),
    }
}

#[inline(always)]
fn unify_structure(
    subs: &mut Subs,
    pool: &mut Pool,
    ctx: &Context,
    flat_type: &FlatType,
    other: &Content,
) -> Outcome {
    match other {
        FlexVar(_) => {
            match &ctx.first_desc.content {
                /*
                Structure(FlatType::Boolean(b)) => match b {
                    Bool::Container(cvar, _mvars)
                        if roc_types::boolean_algebra::var_is_shared(subs, *cvar) =>
                    {
                        subs.set_content(ctx.first, Structure(FlatType::Boolean(Bool::Shared)));
                        subs.set_content(ctx.second, Structure(FlatType::Boolean(Bool::Shared)));

                        vec![]
                    }
                    Bool::Container(_cvar, _mvars) => {
                        merge(subs, ctx, Structure(flat_type.clone()))
                    }
                    Bool::Shared => merge(subs, ctx, Structure(flat_type.clone())),
                },
                    */
                _ => {
                    // If the other is flex, Structure wins!
                    merge(subs, ctx, Structure(flat_type.clone()))
                }
            }
        }
        RigidVar(name) => {
            // Type mismatch! Rigid can only unify with flex.
            mismatch!("trying to unify {:?} with rigid var {:?}", &flat_type, name);
            panic!()
        }

        Structure(ref other_flat_type) => {
            // Unify the two flat types
            unify_flat_type(subs, pool, ctx, flat_type, other_flat_type)
        }
        Alias(_, _, real_var) => unify_pool(subs, pool, ctx.first, *real_var),
        Error => merge(subs, ctx, Error),
    }
}

/// Like intersection_with, except for MutMap and specialized to return
/// a tuple. Also, only clones the values that will be actually returned,
/// rather than cloning everything.
fn get_shared<K, V>(map1: &MutMap<K, V>, map2: &MutMap<K, V>) -> MutMap<K, (V, V)>
where
    K: Clone + Eq + Hash,
    V: Clone,
{
    let mut answer = MutMap::default();

    for (key, right_value) in map2 {
        match std::collections::HashMap::get(map1, &key) {
            None => (),
            Some(left_value) => {
                answer.insert(key.clone(), (left_value.clone(), right_value.clone()));
            }
        }
    }

    answer
}

fn unify_record(
    subs: &mut Subs,
    pool: &mut Pool,
    ctx: &Context,
    rec1: RecordStructure,
    rec2: RecordStructure,
) -> Outcome {
    let fields1 = rec1.fields;
    let fields2 = rec2.fields;
    let shared_fields = get_shared(&fields1, &fields2);
    // NOTE: don't use `difference` here. In contrast to Haskell, im's `difference` is symmetric
    let unique_fields1 = relative_complement(&fields1, &fields2);
    let unique_fields2 = relative_complement(&fields2, &fields1);

    if unique_fields1.is_empty() {
        if unique_fields2.is_empty() {
            let ext_problems = unify_pool(subs, pool, rec1.ext, rec2.ext);

            if !ext_problems.is_empty() {
                return ext_problems;
            }

            let other_fields = MutMap::default();
            let mut field_problems =
                unify_shared_fields(subs, pool, ctx, shared_fields, other_fields, rec1.ext);

            field_problems.extend(ext_problems);

            field_problems
        } else {
            let flat_type = FlatType::Record(unique_fields2, rec2.ext);
            let sub_record = fresh(subs, pool, ctx, Structure(flat_type));
            let ext_problems = unify_pool(subs, pool, rec1.ext, sub_record);

            if !ext_problems.is_empty() {
                return ext_problems;
            }

            let other_fields = MutMap::default();
            let mut field_problems =
                unify_shared_fields(subs, pool, ctx, shared_fields, other_fields, sub_record);

            field_problems.extend(ext_problems);

            field_problems
        }
    } else if unique_fields2.is_empty() {
        let flat_type = FlatType::Record(unique_fields1, rec1.ext);
        let sub_record = fresh(subs, pool, ctx, Structure(flat_type));
        let ext_problems = unify_pool(subs, pool, sub_record, rec2.ext);

        if !ext_problems.is_empty() {
            return ext_problems;
        }

        let other_fields = MutMap::default();
        let mut field_problems =
            unify_shared_fields(subs, pool, ctx, shared_fields, other_fields, sub_record);

        field_problems.extend(ext_problems);

        field_problems
    } else {
        let other_fields = union(unique_fields1.clone(), &unique_fields2);

        let ext = fresh(subs, pool, ctx, Content::FlexVar(None));
        let flat_type1 = FlatType::Record(unique_fields1, ext);
        let flat_type2 = FlatType::Record(unique_fields2, ext);

        let sub1 = fresh(subs, pool, ctx, Structure(flat_type1));
        let sub2 = fresh(subs, pool, ctx, Structure(flat_type2));

        let rec1_problems = unify_pool(subs, pool, rec1.ext, sub2);
        if !rec1_problems.is_empty() {
            return rec1_problems;
        }

        let rec2_problems = unify_pool(subs, pool, sub1, rec2.ext);
        if !rec2_problems.is_empty() {
            return rec2_problems;
        }

        let mut field_problems =
            unify_shared_fields(subs, pool, ctx, shared_fields, other_fields, ext);

        field_problems.reserve(rec1_problems.len() + rec2_problems.len());
        field_problems.extend(rec1_problems);
        field_problems.extend(rec2_problems);

        field_problems
    }
}

fn unify_shared_fields(
    subs: &mut Subs,
    pool: &mut Pool,
    ctx: &Context,
    shared_fields: MutMap<Lowercase, (Variable, Variable)>,
    other_fields: MutMap<Lowercase, Variable>,
    ext: Variable,
) -> Outcome {
    let mut matching_fields = MutMap::default();
    let num_shared_fields = shared_fields.len();

    for (name, (actual, expected)) in shared_fields {
        let problems = unify_pool(subs, pool, actual, expected);

        if problems.is_empty() {
            matching_fields.insert(name, actual);
        }
    }

    if num_shared_fields == matching_fields.len() {
        // pull fields in from the ext_var
        let mut fields = union(matching_fields, &other_fields);

        let new_ext_var = match roc_types::pretty_print::chase_ext_record(subs, ext, &mut fields) {
            Ok(()) => Variable::EMPTY_RECORD,
            Err((new, _)) => new,
        };

        let flat_type = FlatType::Record(fields, new_ext_var);

        merge(subs, ctx, Structure(flat_type))
    } else {
        mismatch!()
    }
}

fn unify_tag_union(
    subs: &mut Subs,
    pool: &mut Pool,
    ctx: &Context,
    rec1: TagUnionStructure,
    rec2: TagUnionStructure,
    recursion: (Option<Variable>, Option<Variable>),
) -> Outcome {
    let tags1 = rec1.tags;
    let tags2 = rec2.tags;
    let shared_tags = get_shared(&tags1, &tags2);
    // NOTE: don't use `difference` here. In contrast to Haskell, im's `difference` is symmetric
    let unique_tags1 = relative_complement(&tags1, &tags2);
    let unique_tags2 = relative_complement(&tags2, &tags1);

    let recursion_var = match recursion {
        (None, None) => None,
        (Some(v), None) | (None, Some(v)) => Some(v),
        (Some(v1), Some(v2)) => {
            unify_pool(subs, pool, v1, v2);
            Some(v1)
        }
    };

    if unique_tags1.is_empty() {
        if unique_tags2.is_empty() {
            let ext_problems = unify_pool(subs, pool, rec1.ext, rec2.ext);

            if !ext_problems.is_empty() {
                return ext_problems;
            }

            let mut tag_problems = unify_shared_tags(
                subs,
                pool,
                ctx,
                shared_tags,
                MutMap::default(),
                rec1.ext,
                recursion_var,
            );

            tag_problems.extend(ext_problems);

            tag_problems
        } else {
            let flat_type = FlatType::TagUnion(unique_tags2, rec2.ext);
            let sub_record = fresh(subs, pool, ctx, Structure(flat_type));
            let ext_problems = unify_pool(subs, pool, rec1.ext, sub_record);

            if !ext_problems.is_empty() {
                return ext_problems;
            }

            let mut tag_problems = unify_shared_tags(
                subs,
                pool,
                ctx,
                shared_tags,
                MutMap::default(),
                sub_record,
                recursion_var,
            );

            tag_problems.extend(ext_problems);

            tag_problems
        }
    } else if unique_tags2.is_empty() {
        let flat_type = FlatType::TagUnion(unique_tags1, rec1.ext);
        let sub_record = fresh(subs, pool, ctx, Structure(flat_type));
        let ext_problems = unify_pool(subs, pool, sub_record, rec2.ext);

        if !ext_problems.is_empty() {
            return ext_problems;
        }

        let mut tag_problems = unify_shared_tags(
            subs,
            pool,
            ctx,
            shared_tags,
            MutMap::default(),
            sub_record,
            recursion_var,
        );

        tag_problems.extend(ext_problems);

        tag_problems
    } else {
        let other_tags = union(unique_tags1.clone(), &unique_tags2);

        let ext = fresh(subs, pool, ctx, Content::FlexVar(None));
        let flat_type1 = FlatType::TagUnion(unique_tags1, ext);
        let flat_type2 = FlatType::TagUnion(unique_tags2, ext);

        let sub1 = fresh(subs, pool, ctx, Structure(flat_type1));
        let sub2 = fresh(subs, pool, ctx, Structure(flat_type2));

        // NOTE: for clearer error messages, we rollback unification of the ext vars when either fails
        //
        // This is inspired by
        //
        //
        //      f : [ Red, Green ] -> Bool
        //      f = \_ -> True
        //
        //      f Blue
        //
        //  In this case, we want the mismatch to be between `[ Blue ]a` and `[ Red, Green ]`, but
        //  without rolling back, the mismatch is between `[ Blue, Red, Green ]a` and `[ Red, Green ]`.
        //  TODO is this also required for the other cases?

        let snapshot = subs.snapshot();

        let ext1_problems = unify_pool(subs, pool, rec1.ext, sub2);
        if !ext1_problems.is_empty() {
            subs.rollback_to(snapshot);
            return ext1_problems;
        }

        let ext2_problems = unify_pool(subs, pool, sub1, rec2.ext);
        if !ext2_problems.is_empty() {
            subs.rollback_to(snapshot);
            return ext2_problems;
        }

        subs.commit_snapshot(snapshot);

        let mut tag_problems =
            unify_shared_tags(subs, pool, ctx, shared_tags, other_tags, ext, recursion_var);

        tag_problems.reserve(ext1_problems.len() + ext2_problems.len());
        tag_problems.extend(ext1_problems);
        tag_problems.extend(ext2_problems);

        tag_problems
    }
}

fn unify_shared_tags(
    subs: &mut Subs,
    pool: &mut Pool,
    ctx: &Context,
    shared_tags: MutMap<TagName, (Vec<Variable>, Vec<Variable>)>,
    other_tags: MutMap<TagName, Vec<Variable>>,
    ext: Variable,
    recursion_var: Option<Variable>,
) -> Outcome {
    let mut matching_tags = MutMap::default();
    let num_shared_tags = shared_tags.len();

    for (name, (actual_vars, expected_vars)) in shared_tags {
        let mut matching_vars = Vec::with_capacity(actual_vars.len());

        let actual_len = actual_vars.len();
        let expected_len = expected_vars.len();

        for (actual, expected) in actual_vars.into_iter().zip(expected_vars.into_iter()) {
            let problems = unify_pool(subs, pool, actual, expected);

            if problems.is_empty() {
                matching_vars.push(actual);
            }
        }

        // only do this check after unification so the error message has more info
        if actual_len == expected_len && actual_len == matching_vars.len() {
            matching_tags.insert(name, matching_vars);
        }
    }

    if num_shared_tags == matching_tags.len() {
        // merge fields from the ext_var into this tag union
        let mut fields = Vec::new();
        let new_ext_var = match roc_types::pretty_print::chase_ext_tag_union(subs, ext, &mut fields)
        {
            Ok(()) => Variable::EMPTY_TAG_UNION,
            Err((new, _)) => new,
        };

        let mut new_tags = union(matching_tags, &other_tags);
        new_tags.extend(fields.into_iter());

        let flat_type = if let Some(rec) = recursion_var {
            FlatType::RecursiveTagUnion(rec, new_tags, new_ext_var)
        } else {
            FlatType::TagUnion(new_tags, new_ext_var)
        };

        merge(subs, ctx, Structure(flat_type))
    } else {
        mismatch!()
    }
}

#[inline(always)]
fn unify_flat_type(
    subs: &mut Subs,
    pool: &mut Pool,
    ctx: &Context,
    left: &FlatType,
    right: &FlatType,
) -> Outcome {
    use roc_types::subs::FlatType::*;

    match (left, right) {
        (EmptyRecord, EmptyRecord) => merge(subs, ctx, Structure(left.clone())),

        (Record(fields, ext), EmptyRecord) if fields.is_empty() => {
            unify_pool(subs, pool, *ext, ctx.second)
        }

        (EmptyRecord, Record(fields, ext)) if fields.is_empty() => {
            unify_pool(subs, pool, ctx.first, *ext)
        }

        (Record(fields1, ext1), Record(fields2, ext2)) => {
            let rec1 = gather_fields(subs, fields1.clone(), *ext1);
            let rec2 = gather_fields(subs, fields2.clone(), *ext2);

            unify_record(subs, pool, ctx, rec1, rec2)
        }

        (EmptyTagUnion, EmptyTagUnion) => merge(subs, ctx, Structure(left.clone())),

        (TagUnion(tags, ext), EmptyTagUnion) if tags.is_empty() => {
            unify_pool(subs, pool, *ext, ctx.second)
        }

        (EmptyTagUnion, TagUnion(tags, ext)) if tags.is_empty() => {
            unify_pool(subs, pool, ctx.first, *ext)
        }

        (TagUnion(tags1, ext1), TagUnion(tags2, ext2)) => {
            let union1 = gather_tags(subs, tags1.clone(), *ext1);
            let union2 = gather_tags(subs, tags2.clone(), *ext2);

            unify_tag_union(subs, pool, ctx, union1, union2, (None, None))
        }

        (TagUnion(tags1, ext1), RecursiveTagUnion(recursion_var, tags2, ext2)) => {
            let union1 = gather_tags(subs, tags1.clone(), *ext1);
            let union2 = gather_tags(subs, tags2.clone(), *ext2);

            unify_tag_union(
                subs,
                pool,
                ctx,
                union1,
                union2,
                (None, Some(*recursion_var)),
            )
        }

        (RecursiveTagUnion(rec1, tags1, ext1), RecursiveTagUnion(rec2, tags2, ext2)) => {
            let union1 = gather_tags(subs, tags1.clone(), *ext1);
            let union2 = gather_tags(subs, tags2.clone(), *ext2);

            unify_tag_union(subs, pool, ctx, union1, union2, (Some(*rec1), Some(*rec2)))
        }

        (Boolean(b1), Boolean(b2)) => {
            use Bool::*;
            match (b1, b2) {
                (Shared, Shared) => merge(subs, ctx, Structure(left.clone())),
                (Shared, Container(cvar, mvars)) => {
                    let mut outcome = vec![];
                    // unify everything with shared
                    outcome.extend(unify_pool(subs, pool, ctx.first, *cvar));

                    for mvar in mvars {
                        outcome.extend(unify_pool(subs, pool, ctx.first, *mvar));
                    }

                    // set the first and second variables to Shared
                    let content = Content::Structure(FlatType::Boolean(Bool::Shared));
                    outcome.extend(merge(subs, ctx, content));

                    outcome
                }
                (Container(cvar, mvars), Shared) => {
                    let mut outcome = vec![];
                    // unify everything with shared
                    outcome.extend(unify_pool(subs, pool, ctx.second, *cvar));

                    for mvar in mvars {
                        outcome.extend(unify_pool(subs, pool, ctx.second, *mvar));
                    }

                    // set the first and second variables to Shared
                    let content = Content::Structure(FlatType::Boolean(Bool::Shared));
                    outcome.extend(merge(subs, ctx, content));

                    outcome
                }
                (Container(cvar1, mvars1), Container(cvar2, mvars2)) => {
                    // unify cvar1 and cvar2?
                    unify_pool(subs, pool, *cvar1, *cvar2);

                    let mvars: SendSet<Variable> = mvars1
                        .into_iter()
                        .chain(mvars2.into_iter())
                        .copied()
                        .filter_map(|v| {
                            let root = subs.get_root_key(v);

                            if roc_types::boolean_algebra::var_is_shared(subs, root) {
                                None
                            } else {
                                Some(root)
                            }
                        })
                        .collect();

                    let content =
                        Content::Structure(FlatType::Boolean(Bool::Container(*cvar1, mvars)));

                    merge(subs, ctx, content)
                }
            }
        }

        (Apply(l_symbol, l_args), Apply(r_symbol, r_args)) if l_symbol == r_symbol => {
            let problems = unify_zip(subs, pool, l_args.iter(), r_args.iter());

            if problems.is_empty() {
                merge(subs, ctx, Structure(Apply(*r_symbol, (*r_args).clone())))
            } else {
                problems
            }
        }
        (Func(l_args, l_ret), Func(r_args, r_ret)) if l_args.len() == r_args.len() => {
            let arg_problems = unify_zip(subs, pool, l_args.iter(), r_args.iter());
            let ret_problems = unify_pool(subs, pool, *l_ret, *r_ret);

            if arg_problems.is_empty() && ret_problems.is_empty() {
                merge(subs, ctx, Structure(Func((*r_args).clone(), *r_ret)))
            } else {
                let mut problems = ret_problems;

                problems.extend(arg_problems);

                problems
            }
        }
        (other1, other2) => mismatch!(
            "Trying to unify two flat types that are incompatible: {:?} ~ {:?}",
            other1,
            other2
        ),
    }
}

fn unify_zip<'a, I>(subs: &mut Subs, pool: &mut Pool, left_iter: I, right_iter: I) -> Outcome
where
    I: Iterator<Item = &'a Variable>,
{
    let mut problems = Vec::new();

    let it = left_iter.zip(right_iter);

    for (&l_var, &r_var) in it {
        problems.extend(unify_pool(subs, pool, l_var, r_var));
    }

    problems
}

#[inline(always)]
fn unify_rigid(subs: &mut Subs, ctx: &Context, name: &Lowercase, other: &Content) -> Outcome {
    match other {
        FlexVar(_) => {
            // If the other is flex, rigid wins!
            merge(subs, ctx, RigidVar(name.clone()))
        }
        RigidVar(_) | Structure(_) | Alias(_, _, _) => {
            // Type mismatch! Rigid can only unify with flex, even if the
            // rigid names are the same.
            mismatch!()
        }
        Error => {
            // Error propagates.
            merge(subs, ctx, Error)
        }
    }
}

#[inline(always)]
fn unify_flex(
    subs: &mut Subs,
    pool: &mut Pool,
    ctx: &Context,
    opt_name: &Option<Lowercase>,
    other: &Content,
) -> Outcome {
    match other {
        FlexVar(None) => {
            // If both are flex, and only left has a name, keep the name around.
            merge(subs, ctx, FlexVar(opt_name.clone()))
        }

        /*
        Structure(FlatType::Boolean(b)) => match b {
            Bool::Container(cvar, _mvars)
                if roc_types::boolean_algebra::var_is_shared(subs, *cvar) =>
            {
                merge(subs, ctx, Structure(FlatType::Boolean(Bool::Shared)))
            }
            Bool::Container(cvar, _mvars) => unify_pool(subs, pool, ctx.first, *cvar),
            Bool::Shared => merge(subs, ctx, other.clone()),
        },
        */
        FlexVar(Some(_)) | RigidVar(_) | Structure(_) | Alias(_, _, _) => {
            // TODO special-case boolean here
            // In all other cases, if left is flex, defer to right.
            // (This includes using right's name if both are flex and named.)
            merge(subs, ctx, other.clone())
        }

        Error => merge(subs, ctx, Error),
    }
}

pub fn merge(subs: &mut Subs, ctx: &Context, content: Content) -> Outcome {
    let rank = ctx.first_desc.rank.min(ctx.second_desc.rank);
    let desc = Descriptor {
        content,
        rank,
        mark: Mark::NONE,
        copy: OptVariable::NONE,
    };

    subs.union(ctx.first, ctx.second, desc);

    Vec::new()
}

fn register(subs: &mut Subs, desc: Descriptor, pool: &mut Pool) -> Variable {
    let var = subs.fresh(desc);

    pool.push(var);

    var
}

fn fresh(subs: &mut Subs, pool: &mut Pool, ctx: &Context, content: Content) -> Variable {
    register(
        subs,
        Descriptor {
            content,
            rank: ctx.first_desc.rank.min(ctx.second_desc.rank),
            mark: Mark::NONE,
            copy: OptVariable::NONE,
        },
        pool,
    )
}

fn gather_tags(
    subs: &mut Subs,
    tags: MutMap<TagName, Vec<Variable>>,
    var: Variable,
) -> TagUnionStructure {
    use roc_types::subs::Content::*;
    use roc_types::subs::FlatType::*;

    match subs.get(var).content {
        Structure(TagUnion(sub_tags, sub_ext)) => {
            gather_tags(subs, union(tags, &sub_tags), sub_ext)
        }

        Alias(_, _, var) => {
            // TODO according to elm/compiler: "TODO may be dropping useful alias info here"
            gather_tags(subs, tags, var)
        }

        _ => TagUnionStructure { tags, ext: var },
    }
}

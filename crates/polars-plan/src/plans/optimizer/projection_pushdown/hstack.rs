use super::*;

#[allow(clippy::too_many_arguments)]
pub(super) fn process_hstack(
    proj_pd: &mut ProjectionPushDown,
    input: Node,
    mut exprs: Vec<ExprIR>,
    options: ProjectionOptions,
    mut ctx: ProjectionContext,
    lp_arena: &mut Arena<IR>,
    expr_arena: &mut Arena<AExpr>,
) -> PolarsResult<IR> {
    if ctx.has_pushed_down() {
        let mut pruned_with_cols = Vec::with_capacity(exprs.len());

        // Check if output names are used upstream
        // if not, we can prune the `with_column` expression
        // as it is not used in the output.
        for e in exprs {
            let is_used_upstream = ctx.projected_names.contains(e.output_name());
            if is_used_upstream {
                pruned_with_cols.push(e);
            }
        }

        if pruned_with_cols.is_empty() {
            proj_pd.pushdown_and_assign(input, ctx, lp_arena, expr_arena)?;
            return Ok(lp_arena.take(input));
        }

        // Make sure that columns selected with_columns are available
        // only if not empty. If empty we already select everything.
        for e in &pruned_with_cols {
            add_expr_to_accumulated(
                e.node(),
                &mut ctx.acc_projections,
                &mut ctx.projected_names,
                expr_arena,
            );
        }

        exprs = pruned_with_cols
    }
    // projections that select columns added by
    // this `with_column` operation can be dropped
    // For instance in:
    //
    //  q
    //  .with_column(col("a").alias("b")
    //  .select(["a", "b"])
    //
    // we can drop the "b" projection at this level
    let (acc_projections, _, names) = split_acc_projections(
        ctx.acc_projections,
        &lp_arena.get(input).schema(lp_arena),
        expr_arena,
        true, // expands_schema
    );

    let ctx = ProjectionContext::new(acc_projections, names, ctx.inner);
    proj_pd.pushdown_and_assign(input, ctx, lp_arena, expr_arena)?;

    let lp = IRBuilder::new(input, expr_arena, lp_arena)
        .with_columns(exprs, options)
        .build();
    Ok(lp)
}

use anyhow::Result;

/// 启动占位 REST 服务（暂未实现）。
pub async fn serve(dry_run: bool) -> Result<()> {
    if dry_run {
        log::info!("[dry-run] REST 服务未真正启动");
        return Ok(());
    }

    log::info!("REST 服务骨架尚未实现，后续阶段将接入实际路由");
    Ok(())
}

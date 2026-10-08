-- 管理端按节点分页列表：node_id 过滤 + created_at/id 排序
CREATE INDEX IF NOT EXISTS tunnels_node_admin_list_idx
    ON tunnels (node_id, created_at DESC, id DESC)
    WHERE status <> 'deleted';

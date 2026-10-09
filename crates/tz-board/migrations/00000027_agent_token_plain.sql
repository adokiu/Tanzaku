-- 节点 / Client token 保存明文以便在面板再次查看与生成安装命令；认证仍只比对 token_hash。
-- 旧记录为 NULL，需在面板重置 token 后才能查看。
ALTER TABLE nodes ADD COLUMN IF NOT EXISTS token TEXT;
ALTER TABLE clients ADD COLUMN IF NOT EXISTS token TEXT;

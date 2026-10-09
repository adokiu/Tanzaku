-- 域名不再全局唯一：同一用户可在不同入口（共享 80/443 与各独立端口）重复绑定同一域名。
-- 冲突规则改由应用层在节点端口锁内校验：同节点同入口不能重复，其他用户占用中的域名不能绑定。
ALTER TABLE tunnel_domains DROP CONSTRAINT IF EXISTS tunnel_domains_domain_key;
CREATE INDEX IF NOT EXISTS tunnel_domains_domain_idx ON tunnel_domains (domain);
CREATE UNIQUE INDEX IF NOT EXISTS tunnel_domains_tunnel_domain_uidx ON tunnel_domains (tunnel_id, domain);

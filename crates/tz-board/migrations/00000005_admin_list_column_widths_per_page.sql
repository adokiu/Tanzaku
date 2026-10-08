INSERT INTO system_settings (key, value) VALUES
(
  'admin_list_column_widths_nodes',
  '{"id":"80px","online":"68px","region":"56px","public_host":"93px","name":"minmax(88px, 0.6fr)","cpu":"168px","memory":"168px","bandwidth":"140px","tunnel_count":"88px","last_seen_at":"168px","actions":"220px"}'::jsonb
),
(
  'admin_list_column_widths_clients',
  '{"id":"80px","online":"68px","region":"56px","public_ip":"93px","name":"minmax(88px, 0.6fr)","user_email":"minmax(160px, 1fr)","system":"200px","traffic_in":"100px","traffic_out":"100px","traffic_speed":"140px","tunnel_count":"72px","last_seen_at":"168px","actions":"220px"}'::jsonb
)
ON CONFLICT (key) DO NOTHING;

DELETE FROM system_settings WHERE key = 'admin_list_column_widths';

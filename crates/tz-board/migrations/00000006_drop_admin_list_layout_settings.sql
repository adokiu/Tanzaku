DELETE FROM system_settings WHERE key IN (
  'admin_list_column_gap',
  'admin_list_column_widths',
  'admin_list_column_widths_nodes',
  'admin_list_column_widths_clients',
  'admin_page_max_width',
  'admin_border_radius'
);

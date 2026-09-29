-- The marketplace code a plugin was installed from, so an employee that
-- names the plugin by that code (`requires.plugins: ["PLUG-…"]`) reaches the
-- installed plugin's own tool. Empty: installed without a code, or before
-- codes were recorded.
ALTER TABLE plugin_registry ADD COLUMN install_code TEXT NOT NULL DEFAULT '';

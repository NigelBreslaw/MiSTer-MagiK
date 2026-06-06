#pragma once

void mister_magik_launcher_cfg_apply(void);
bool mister_magik_launcher_configured(void);
bool mister_magik_launcher_active(void);
void mister_magik_launcher_init_for_menu(void);
void mister_magik_launcher_poll(void);
void mister_magik_launcher_shutdown(void);
bool mister_magik_boot_analytics_enabled(void);
void mister_magik_boot_analytics_event(const char *source, const char *event, const char *fmt, ...);

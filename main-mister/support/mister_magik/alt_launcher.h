#pragma once

void mister_magik_launcher_cfg_apply(void);
bool mister_magik_launcher_configured(void);
bool mister_magik_launcher_active(void);
void mister_magik_launcher_init_for_menu(void);
void mister_magik_launcher_poll(void);
void mister_magik_launcher_shutdown(void);
void mister_magik_launcher_exit_to_menu(void);
bool mister_magik_boot_analytics_enabled(void);
void mister_magik_boot_analytics_event(const char *source, const char *event, const char *fmt, ...);
void mister_magik_status_write(void);
void mister_magik_note_visible_owner(const char *owner, int fb_enabled, int fb_num, int fb_width, int fb_height);
void mister_magik_note_osd_suppressed(void);

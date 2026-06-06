#pragma once

void mm_launcher_cfg_apply(void);
void mm_launcher_init_for_menu(void);
void mm_launcher_poll(void);
void mm_launcher_shutdown(void);
void mm_launcher_prepare_for_launch(void);
void mm_launcher_yield_for_osd(unsigned short key, int press);
bool mm_launcher_handle_osd_key(unsigned short key, int press);
bool mm_launcher_active(void);
bool mm_launcher_configured(void);
bool mm_launcher_suppresses_stock_osd(void);
bool mm_launcher_stock_osd_active(void);

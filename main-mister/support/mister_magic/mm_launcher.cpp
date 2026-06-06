#include "mm_launcher.h"

#include <errno.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <linux/input.h>
#include <sys/wait.h>

#include "cfg.h"
#include "hardware.h"
#include "menu.h"
#include "osd.h"
#include "video.h"

char user_io_osd_is_visible(void);

static const char s_launcher_path[] = "/media/fat/mister-magic/mister-magic-fb";
static const char s_log_path[] = "/tmp/mister-magic-main.log";
static pid_t s_pid = 0;
static int s_crash_count = 0;
static unsigned long s_respawn_timer = 0;
static bool s_init_pending = false;
static bool s_gave_up = false;
static bool s_overlay_visible = false;
static int s_overlay_selected = 0;
static bool s_stock_osd_active = false;
static bool s_stock_osd_was_visible = false;

static const char *s_overlay_items[] = {
	"Show MiSTer Menu",
	"Return to MiSTer Magik",
};
static const int s_overlay_item_count = sizeof(s_overlay_items) / sizeof(s_overlay_items[0]);

static void log_line(const char *msg)
{
	FILE *f = fopen(s_log_path, "a");
	if (!f) return;
	fprintf(f, "%s\n", msg);
	fclose(f);
}

static void log_errno(const char *prefix)
{
	FILE *f = fopen(s_log_path, "a");
	if (!f) return;
	fprintf(f, "%s: %s\n", prefix, strerror(errno));
	fclose(f);
}

static void focus_slint_framebuffer()
{
	video_chvt(2);
	video_fb_enable(1);
	if (menu_present()) MenuHide();
	OsdDisable();
}

void mm_launcher_cfg_apply(void)
{
	// Experimental Main-as-parent mode needs the Linux framebuffer path alive.
	cfg.fb_terminal = 1;
	cfg.recents = 1;
	log_line("cfg_apply");
}

bool mm_launcher_configured(void)
{
	bool configured = access(s_launcher_path, X_OK) == 0;
	if (!configured) log_errno("launcher access failed");
	return configured;
}

bool mm_launcher_active(void)
{
	return s_pid != 0;
}

bool mm_launcher_suppresses_stock_osd(void)
{
	if (s_stock_osd_active)
		return false;
	return s_pid != 0 || s_init_pending || s_overlay_visible;
}

bool mm_launcher_stock_osd_active(void)
{
	return s_stock_osd_active;
}

static void kill_launcher(pid_t pid, int sig)
{
	if (kill(-pid, sig) && errno == ESRCH)
		kill(pid, sig);
}

static void wait_launcher_stopped(pid_t pid)
{
	kill_launcher(pid, SIGTERM);
	for (int i = 0; i < 50; i++)
	{
		if (waitpid(pid, NULL, WNOHANG) == pid)
		{
			s_pid = 0;
			return;
		}
		usleep(10000);
	}

	kill_launcher(pid, SIGKILL);
	for (int i = 0; i < 100; i++)
	{
		if (waitpid(pid, NULL, WNOHANG) == pid)
		{
			s_pid = 0;
			return;
		}
		usleep(10000);
	}
}

static void spawn(void)
{
	if (!mm_launcher_configured())
	{
		printf("mister_magik: launcher not executable: %s\n", s_launcher_path);
		log_line("spawn gave up: launcher not executable");
		s_gave_up = true;
		return;
	}

	// If Main was re-entered/restarted, an old launcher child can survive with
	// no matching pid in this process. Clear stale children before forking.
	system("kill -TERM $(pidof mister-magic-fb) 2>/dev/null");
	usleep(100000);

	printf("mister_magik: spawning Slint launcher UI: %s\n", s_launcher_path);
	log_line("spawn");
	s_pid = fork();
	if (s_pid < 0)
	{
		printf("mister_magik: fork failed: %s\n", strerror(errno));
		log_errno("fork failed");
		s_pid = 0;
		s_gave_up = true;
		return;
	}

	if (!s_pid)
	{
		setsid();
		setenv("MISTER_MAGIK_PARENT", "main-mister", 1);
		execl(s_launcher_path, s_launcher_path, "ui", "launcher", "0", NULL);
		_exit(127);
	}

	focus_slint_framebuffer();
}

static void draw_overlay_osd()
{
	int rows = OsdGetSize();
	if (rows < 8) rows = 8;

	OsdClear();
	OsdSetTitle("MiSTer Magik");
	for (int i = 0; i < rows; i++) OsdWrite(i);

	OsdWrite(4,  "        Show MiSTer Menu", s_overlay_selected == 0);
	OsdWrite(6,  "        Return to MiSTer Magik", s_overlay_selected == 1);
	OsdWrite(rows - 1, "      D-pad moves, A selects");
	OsdUpdate();
	OsdEnable(DISABLE_KEYBOARD);
}

static void close_overlay_osd()
{
	OsdDisable();
	s_overlay_visible = false;
	focus_slint_framebuffer();
}

static void show_stock_mister_menu()
{
	FILE *f = fopen(s_log_path, "a");
	if (f)
	{
		fprintf(f, "osd stock menu\n");
		fclose(f);
	}

	OsdDisable();
	s_overlay_visible = false;
	s_stock_osd_active = true;
	s_stock_osd_was_visible = false;
	video_fb_enable(0);
	menu_key_set(KEY_F12);
}

void mm_launcher_init_for_menu(void)
{
	log_line("init_for_menu");
	if (s_pid || s_gave_up)
		return;
	s_crash_count = 0;
	s_respawn_timer = 0;
	s_init_pending = true;
}

void mm_launcher_poll(void)
{
	if (s_pid)
	{
		if (s_stock_osd_active)
		{
			if (user_io_osd_is_visible())
			{
				s_stock_osd_was_visible = true;
			}
			else if (s_stock_osd_was_visible)
			{
				log_line("osd stock menu closed");
				s_stock_osd_active = false;
				s_stock_osd_was_visible = false;
				focus_slint_framebuffer();
			}
		}

		int status = 0;
		if (waitpid(s_pid, &status, WNOHANG) == s_pid)
		{
			s_pid = 0;
			bool exited = WIFEXITED(status);
			int exit_status = exited ? WEXITSTATUS(status) : 0;
			int sig = WIFSIGNALED(status) ? WTERMSIG(status) : 0;
			bool clean = (exited && exit_status == 0) || sig == SIGTERM || sig == SIGINT;

			printf("mister_magik: launcher exited clean=%d status=%d sig=%d\n",
			       clean ? 1 : 0, exit_status, sig);

			if (!clean && ++s_crash_count < 3)
			{
				s_respawn_timer = GetTimer(1000);
				if (!s_respawn_timer) s_respawn_timer = 1;
			}
			else
			{
				s_gave_up = true;
				video_fb_enable(0);
			}
		}
		return;
	}

	if (s_init_pending)
	{
		log_line("poll init pending");
		s_init_pending = false;
		spawn();
		return;
	}

	if (s_respawn_timer && CheckTimer(s_respawn_timer))
	{
		s_respawn_timer = 0;
		spawn();
	}
}

void mm_launcher_shutdown(void)
{
	log_line("shutdown");
	if (s_pid)
		wait_launcher_stopped(s_pid);
	s_init_pending = false;
	s_respawn_timer = 0;
	s_overlay_visible = false;
	s_overlay_selected = 0;
	s_stock_osd_active = false;
	s_stock_osd_was_visible = false;
	OsdDisable();
	video_fb_enable(0);
}

void mm_launcher_prepare_for_launch(void)
{
	printf("mister_magik: preparing Main-owned launch\n");
	log_line("prepare_for_launch");
	mm_launcher_shutdown();
	if (menu_present()) MenuHide();
}

void mm_launcher_yield_for_osd(unsigned short key, int press)
{
	if (!s_pid)
		return;
	if (s_stock_osd_active)
		return;
	if (press != 1)
		return;

	FILE *f = fopen(s_log_path, "a");
	if (f)
	{
		fprintf(f, "osd overlay key=%u press=%d visible=%d overlay=%d\n",
		        key, press, user_io_osd_is_visible() ? 1 : 0, s_overlay_visible ? 1 : 0);
		fclose(f);
	}

	if (s_overlay_visible)
	{
		close_overlay_osd();
		return;
	}

	focus_slint_framebuffer();
	s_overlay_selected = 0;
	draw_overlay_osd();
	s_overlay_visible = true;
}

bool mm_launcher_handle_osd_key(unsigned short key, int press)
{
	if (!s_overlay_visible)
		return false;

	if (key != KEY_UP && key != KEY_DOWN && key != KEY_LEFT && key != KEY_RIGHT &&
	    key != KEY_ENTER && key != KEY_SPACE && key != KEY_ESC &&
	    key != KEY_MENU && key != KEY_F12)
		return false;

	if (press != 1)
		return true;

	FILE *f = fopen(s_log_path, "a");
	if (f)
	{
		fprintf(f, "osd overlay nav key=%u selected=%d\n", key, s_overlay_selected);
		fclose(f);
	}

	switch (key)
	{
	case KEY_UP:
	case KEY_LEFT:
		s_overlay_selected = (s_overlay_selected + s_overlay_item_count - 1) % s_overlay_item_count;
		draw_overlay_osd();
		break;
	case KEY_DOWN:
	case KEY_RIGHT:
		s_overlay_selected = (s_overlay_selected + 1) % s_overlay_item_count;
		draw_overlay_osd();
		break;
	case KEY_ENTER:
	case KEY_SPACE:
		if (s_overlay_selected == 0)
			show_stock_mister_menu();
		else
			close_overlay_osd();
		break;
	case KEY_ESC:
	case KEY_MENU:
	case KEY_F12:
		close_overlay_osd();
		break;
	}

	return true;
}

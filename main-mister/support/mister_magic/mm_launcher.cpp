#include "mm_launcher.h"

#include <errno.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <sys/wait.h>

#include "cfg.h"
#include "hardware.h"
#include "menu.h"
#include "video.h"

char user_io_osd_is_visible(void);

static const char s_launcher_path[] = "/media/fat/mister-magic/mister-magic-fb";
static const char s_log_path[] = "/tmp/mister-magic-main.log";
static pid_t s_pid = 0;
static int s_crash_count = 0;
static unsigned long s_respawn_timer = 0;
static unsigned long s_osd_restore_timer = 0;
static bool s_init_pending = false;
static bool s_gave_up = false;
static bool s_yielded_for_osd = false;

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
		printf("mister_magic: launcher not executable: %s\n", s_launcher_path);
		log_line("spawn gave up: launcher not executable");
		s_gave_up = true;
		return;
	}

	printf("mister_magic: spawning Slint debug UI: %s\n", s_launcher_path);
	log_line("spawn");
	s_pid = fork();
	if (s_pid < 0)
	{
		printf("mister_magic: fork failed: %s\n", strerror(errno));
		log_errno("fork failed");
		s_pid = 0;
		s_gave_up = true;
		return;
	}

	if (!s_pid)
	{
		setsid();
		setenv("MISTER_MAGIC_PARENT", "main-mister", 1);
		execl(s_launcher_path, s_launcher_path, "ui", "debug", "86400", NULL);
		_exit(127);
	}

	video_chvt(2);
	video_fb_enable(1);
	if (menu_present()) MenuHide();
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
		if (s_yielded_for_osd && CheckTimer(s_osd_restore_timer) && !user_io_osd_is_visible())
		{
			log_line("osd yield restore");
			s_yielded_for_osd = false;
			s_osd_restore_timer = 0;
			video_chvt(2);
			video_fb_enable(1);
			if (menu_present()) MenuHide();
		}

		int status = 0;
		if (waitpid(s_pid, &status, WNOHANG) == s_pid)
		{
			s_pid = 0;
			bool exited = WIFEXITED(status);
			int exit_status = exited ? WEXITSTATUS(status) : 0;
			int sig = WIFSIGNALED(status) ? WTERMSIG(status) : 0;
			bool clean = (exited && exit_status == 0) || sig == SIGTERM || sig == SIGINT;

			printf("mister_magic: launcher exited clean=%d status=%d sig=%d\n",
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
	s_osd_restore_timer = 0;
	s_yielded_for_osd = false;
	video_fb_enable(0);
}

void mm_launcher_prepare_for_launch(void)
{
	printf("mister_magic: preparing Main-owned launch\n");
	log_line("prepare_for_launch");
	mm_launcher_shutdown();
	if (menu_present()) MenuHide();
}

void mm_launcher_yield_for_osd(unsigned short key, int press)
{
	if (!s_pid)
		return;

	FILE *f = fopen(s_log_path, "a");
	if (f)
	{
		fprintf(f, "osd yield key=%u press=%d visible=%d\n",
		        key, press, user_io_osd_is_visible() ? 1 : 0);
		fclose(f);
	}

	s_yielded_for_osd = true;
	s_osd_restore_timer = GetTimer(1500);
	if (!s_osd_restore_timer) s_osd_restore_timer = 1;
	video_fb_enable(0);
}

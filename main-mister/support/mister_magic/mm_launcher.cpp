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

static const char s_launcher_path[] = "/media/fat/mister-magic/mister-magic-fb";
static pid_t s_pid = 0;
static int s_crash_count = 0;
static unsigned long s_respawn_timer = 0;
static bool s_init_pending = false;
static bool s_gave_up = false;

void mm_launcher_cfg_apply(void)
{
	// Experimental Main-as-parent mode needs the Linux framebuffer path alive.
	cfg.fb_terminal = 1;
	cfg.recents = 1;
}

bool mm_launcher_configured(void)
{
	return access(s_launcher_path, X_OK) == 0;
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
		s_gave_up = true;
		return;
	}

	printf("mister_magic: spawning Slint debug UI: %s\n", s_launcher_path);
	s_pid = fork();
	if (s_pid < 0)
	{
		printf("mister_magic: fork failed: %s\n", strerror(errno));
		s_pid = 0;
		s_gave_up = true;
		return;
	}

	if (!s_pid)
	{
		setsid();
		setenv("MISTER_MAGIC_PARENT", "main-mister", 1);
		execl(s_launcher_path, s_launcher_path, "ui", "debug", "0", NULL);
		_exit(127);
	}

	video_chvt(2);
	video_fb_enable(1);
	if (menu_present()) MenuHide();
}

void mm_launcher_init_for_menu(void)
{
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
	if (s_pid)
		wait_launcher_stopped(s_pid);
	s_init_pending = false;
	s_respawn_timer = 0;
	video_fb_enable(0);
}

void mm_launcher_prepare_for_launch(void)
{
	printf("mister_magic: preparing Main-owned launch\n");
	mm_launcher_shutdown();
	if (menu_present()) MenuHide();
}

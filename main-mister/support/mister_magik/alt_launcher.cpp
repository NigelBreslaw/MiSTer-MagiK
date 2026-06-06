#include "alt_launcher.h"
#include <errno.h>
#include <fcntl.h>
#include <sched.h>
#include <signal.h>
#include <stdarg.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <termios.h>
#include <unistd.h>
#include <linux/kd.h>
#include <linux/vt.h>
#include <sys/ioctl.h>
#include <sys/wait.h>
#include "cfg.h"
#include "file_io.h"
#include "hardware.h"
#include "menu.h"
#include "user_io.h"
#include "video.h"

static const char s_launcher_path[] = "mister-magik/mister-magik-fb";
static const char s_launcher_scene[] = "launcher";
static const char s_script_path[] = "/tmp/mister_magik_launcher";
static const char s_log_path[] = "/tmp/mister-magik-main.log";
static const int s_vt = 2;
static const char s_tty[] = "tty2";
static const char s_tty_path[] = "/dev/tty2";

static pid_t s_pid = 0;
static int s_crash_count = 0;
static unsigned long s_respawn_timer = 0;
static bool s_init_pending = false;
static bool s_gave_up = false;
static bool s_escaped = false;

static void log_msg(const char *fmt, ...)
{
	FILE *f = fopen(s_log_path, "a");
	if (!f) return;

	va_list args;
	va_start(args, fmt);
	vfprintf(f, fmt, args);
	va_end(args);
	fputc('\n', f);
	fclose(f);
}

bool mister_magik_launcher_configured(void)
{
	if (s_escaped) return false;

	static int cached = -1;
	if (cached < 0) cached = FileExists(s_launcher_path, 0) ? 1 : 0;
	return cached != 0;
}

void mister_magik_launcher_cfg_apply(void)
{
	if (!mister_magik_launcher_configured())
		return;

	// Same production assumption as Zaparoo: the fork is single-purpose while
	// the frontend is installed, so keep the Linux framebuffer path available.
	cfg.fb_terminal = 1;
	cfg.recents = 1;
}

static void clear_launcher_tty(void)
{
	int tty_fd = open(s_tty_path, O_WRONLY | O_CLOEXEC);
	if (tty_fd >= 0)
	{
		static const char blank[] = "\033[?25l\033[40m\033[30m\033[2J\033[H";
		if (write(tty_fd, blank, sizeof(blank) - 1) < 0) {}
		close(tty_fd);
	}
}

static void reset_launcher_tty(void)
{
	int tty_fd = open(s_tty_path, O_RDWR | O_NOCTTY | O_CLOEXEC);
	if (tty_fd >= 0)
	{
		ioctl(tty_fd, KDSETMODE, KD_TEXT);
		ioctl(tty_fd, KDSKBMODE, K_XLATE);

		struct vt_mode vtmode;
		memset(&vtmode, 0, sizeof(vtmode));
		vtmode.mode = VT_AUTO;
		ioctl(tty_fd, VT_SETMODE, &vtmode);

		struct termios tio;
		if (!tcgetattr(tty_fd, &tio))
		{
			tio.c_iflag |= BRKINT | ICRNL | IXON | IMAXBEL;
			tio.c_iflag &= ~(IGNBRK | INLCR | IGNCR | IXOFF);
			tio.c_oflag |= OPOST | ONLCR;
			tio.c_lflag |= ISIG | ICANON | ECHO | ECHOE | ECHOK | IEXTEN;
			tio.c_lflag &= ~(NOFLSH | TOSTOP);
			tio.c_cflag |= CREAD;
			tio.c_cc[VMIN] = 1;
			tio.c_cc[VTIME] = 0;
			tcsetattr(tty_fd, TCSANOW, &tio);
		}

		static const char reset[] = "\033[0m\033[?25h\033[37m\033[40m\033[2J\033[H";
		if (write(tty_fd, reset, sizeof(reset) - 1) < 0) {}
		close(tty_fd);
	}
}

static bool launcher_tty_ready(pid_t pid)
{
	char fd_path[64];
	char target[128];

	for (int fd = 0; fd < 3; fd++)
	{
		snprintf(fd_path, sizeof(fd_path), "/proc/%d/fd/%d", pid, fd);
		ssize_t len = readlink(fd_path, target, sizeof(target) - 1);
		if (len > 0)
		{
			target[len] = 0;
			if (!strcmp(target, s_tty_path))
				return true;
		}
	}

	return false;
}

static void wait_launcher_tty_ready(pid_t pid)
{
	for (int i = 0; i < 100; i++)
	{
		if (launcher_tty_ready(pid))
			return;
		usleep(10000);
	}
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

static void return_to_normal_mode(void)
{
	user_io_osd_key_enable(1);
	reset_launcher_tty();
	video_fb_enable(0);
	video_menu_bg(user_io_status_get("[3:1]"));
	s_respawn_timer = 0;
	s_crash_count = 0;
	s_gave_up = true;
	s_escaped = true;
	log_msg("return_to_normal_mode");
}

static bool write_launcher_script(void)
{
	static const char cmd[] =
		"#!/bin/bash\n"
		"export LC_ALL=en_US.UTF-8\n"
		"export HOME=/root\n"
		"export MISTER_MAGIK_PARENT=main-mister\n"
		"printf '\\033[0m\\033[?25l\\033[37m\\033[40m\\033[2J\\033[H'\n"
		"exec \"$MISTER_MAGIK_PATH\" ui \"$MISTER_MAGIK_SCENE\" 0\n";

	unlink(s_script_path);
	return FileSave(s_script_path, (void*)cmd, strlen(cmd)) != 0;
}

static void spawn(void)
{
	char path[2100];
	strncpy(path, getFullPath(s_launcher_path), sizeof(path) - 1);
	path[sizeof(path) - 1] = '\0';

	if (!FileExists(s_launcher_path, 0))
	{
		log_msg("spawn skipped: missing %s", s_launcher_path);
		return_to_normal_mode();
		return;
	}

	if (!write_launcher_script())
	{
		log_msg("spawn failed: unable to write %s", s_script_path);
		return_to_normal_mode();
		return;
	}

	user_io_osd_key_enable(0);
	clear_launcher_tty();

	s_pid = fork();
	if (s_pid < 0)
	{
		log_msg("fork failed: %s", strerror(errno));
		s_pid = 0;
		user_io_osd_key_enable(1);
		video_fb_enable(0);
		return;
	}

	if (!s_pid)
	{
		setenv("MISTER_MAGIK_PATH", path, 1);
		setenv("MISTER_MAGIK_SCENE", s_launcher_scene, 1);
		cpu_set_t set;
		CPU_ZERO(&set);
		CPU_SET(0, &set);
		sched_setaffinity(0, sizeof(set), &set);
		setsid();
		execl("/sbin/agetty", "/sbin/agetty", "-a", "root", "-l",
		      s_script_path, "-i", "--nohostname", "-L", s_tty, "linux", NULL);
		_exit(1);
	}

	log_msg("spawned pid=%d path=%s scene=%s", s_pid, path, s_launcher_scene);
	wait_launcher_tty_ready(s_pid);
	video_chvt(s_vt);
	video_fb_enable(1);
	if (menu_present()) MenuHide();
}

bool mister_magik_launcher_active(void)
{
	return s_pid != 0;
}

void mister_magik_launcher_init_for_menu(void)
{
	if (!mister_magik_launcher_configured() || s_pid || s_gave_up)
		return;

	s_crash_count = 0;
	s_respawn_timer = 0;
	s_init_pending = true;
	log_msg("init_for_menu");
}

void mister_magik_launcher_poll(void)
{
	if (s_pid)
	{
		int status;
		if (waitpid(s_pid, &status, WNOHANG) == s_pid)
		{
			s_pid = 0;
			user_io_osd_key_enable(1);

			bool exited = WIFEXITED(status);
			int exit_status = exited ? WEXITSTATUS(status) : 0;
			int sig = WIFSIGNALED(status) ? WTERMSIG(status) : 0;
			bool escaped = (exited && exit_status == 0) || sig == SIGTERM || sig == SIGINT;
			bool crashed = !escaped && (sig != 0 || (exited && exit_status != 0));

			log_msg("launcher exited escaped=%d crashed=%d status=%d sig=%d",
			        escaped, crashed, exit_status, sig);
			reset_launcher_tty();

			if (escaped)
			{
				return_to_normal_mode();
				return;
			}

			if (crashed && ++s_crash_count >= 3)
			{
				log_msg("giving up after 3 crashes");
				return_to_normal_mode();
				return;
			}

			if (!crashed)
				s_crash_count = 0;

			s_respawn_timer = GetTimer(1000);
			if (!s_respawn_timer) s_respawn_timer = 1;
		}
		return;
	}

	if (!mister_magik_launcher_configured())
		return;

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

void mister_magik_launcher_shutdown(void)
{
	if (s_pid)
		wait_launcher_stopped(s_pid);

	s_pid = 0;
	s_respawn_timer = 0;
	s_crash_count = 0;
	s_init_pending = false;
	s_gave_up = false;
	s_escaped = false;
	user_io_osd_key_enable(1);
	video_fb_enable(0);
	reset_launcher_tty();
	log_msg("shutdown");
}

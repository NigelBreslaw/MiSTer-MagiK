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
#include <sys/stat.h>
#include <sys/wait.h>
#include "cfg.h"
#include "file_io.h"
#include "hardware.h"
#include "menu.h"
#include "osd.h"
#include "user_io.h"
#include "video.h"

static const char s_launcher_path[] = "mister-magik/mister-magik-fb";
static const char s_launcher_scene[] = "launcher";
static const char s_script_path[] = "/tmp/mister_magik_launcher";
static const char s_log_path[] = "/tmp/mister-magik-main.log";
static const char s_status_dir[] = "/tmp/mister-magik";
static const char s_status_path[] = "/tmp/mister-magik/main-status.json";
static const char s_events_path[] = "/tmp/mister-magik/events.jsonl";
static const char s_analytics_flag_path[] = "/media/fat/mister-magik/boot-analytics.enabled";
static const char s_analytics_path[] = "/tmp/mister-magik-boot-analytics.tsv";
static const int s_vt = 2;
static const char s_tty[] = "tty2";
static const char s_tty_path[] = "/dev/tty2";

static pid_t s_pid = 0;
static int s_crash_count = 0;
static unsigned long s_respawn_timer = 0;
static bool s_init_pending = false;
static bool s_gave_up = false;
static bool s_escaped = false;
static unsigned long s_analytics_seq = 0;
static bool s_analytics_header_written = false;
static unsigned long s_osd_suppressed_count = 0;
static char s_visible_owner[32] = "unknown";
static int s_visible_fb_enabled = 0;
static int s_visible_fb_num = -1;
static int s_visible_fb_width = 0;
static int s_visible_fb_height = 0;

static bool launcher_tty_ready(pid_t pid);
static void json_escape(FILE *f, const char *s);
static void read_trimmed(const char *path, char *buf, size_t len);

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

static void ensure_status_dir(void)
{
	mkdir(s_status_dir, 0755);
}

static void json_escape(FILE *f, const char *s)
{
	fputc('"', f);
	if (s)
	{
		for (; *s; s++)
		{
			switch (*s)
			{
			case '\\': fputs("\\\\", f); break;
			case '"': fputs("\\\"", f); break;
			case '\n': fputs("\\n", f); break;
			case '\r': fputs("\\r", f); break;
			case '\t': fputs("\\t", f); break;
			default: fputc(*s, f); break;
			}
		}
	}
	fputc('"', f);
}

static void event_jsonl(const char *source, const char *event, const char *detail)
{
	ensure_status_dir();
	FILE *f = fopen(s_events_path, "a");
	if (!f) return;
	fprintf(f, "{\"ts_boot_ms\":%lu,\"source\":", GetTimer(0));
	json_escape(f, source ? source : "main");
	fprintf(f, ",\"pid\":%d,\"event\":", getpid());
	json_escape(f, event ? event : "unknown");
	fprintf(f, ",\"detail\":");
	json_escape(f, detail ? detail : "");
	fprintf(f, "}\n");
	fclose(f);
}

void mister_magik_status_write(void)
{
	ensure_status_dir();
	FILE *f = fopen(s_status_path, "w");
	if (!f) return;

	char active_vt[64];
	char fb_mode[128];
	read_trimmed("/sys/class/tty/tty0/active", active_vt, sizeof(active_vt));
	read_trimmed("/sys/module/MiSTer_fb/parameters/mode", fb_mode, sizeof(fb_mode));

	fprintf(f, "{");
	fprintf(f, "\"schema\":\"mister-magik-main-status-v1\",");
	fprintf(f, "\"ts_boot_ms\":%lu,", GetTimer(0));
	fprintf(f, "\"pid\":%d,", getpid());
	fprintf(f, "\"launcher_pid\":%d,", s_pid);
	fprintf(f, "\"launcher_active\":%s,", s_pid ? "true" : "false");
	fprintf(f, "\"crash_count\":%d,", s_crash_count);
	fprintf(f, "\"respawn_timer\":%lu,", s_respawn_timer);
	fprintf(f, "\"init_pending\":%s,", s_init_pending ? "true" : "false");
	fprintf(f, "\"gave_up\":%s,", s_gave_up ? "true" : "false");
	fprintf(f, "\"escaped\":%s,", s_escaped ? "true" : "false");
	fprintf(f, "\"tty_ready\":%s,", (s_pid && launcher_tty_ready(s_pid)) ? "true" : "false");
	fprintf(f, "\"active_vt\":");
	json_escape(f, active_vt[0] ? active_vt : "unknown");
	fprintf(f, ",\"fb_mode\":");
	json_escape(f, fb_mode[0] ? fb_mode : "unknown");
	fprintf(f, ",\"visible_owner\":");
	json_escape(f, s_visible_owner);
	fprintf(f, ",\"fb_enabled\":%d,\"fb_num\":%d,\"fb_width\":%d,\"fb_height\":%d,",
	        s_visible_fb_enabled, s_visible_fb_num, s_visible_fb_width, s_visible_fb_height);
	fprintf(f, "\"osd_suppressed_count\":%lu", s_osd_suppressed_count);
	fprintf(f, "}\n");
	fclose(f);
}

void mister_magik_note_visible_owner(const char *owner, int fb_enabled, int fb_num, int fb_width, int fb_height)
{
	snprintf(s_visible_owner, sizeof(s_visible_owner), "%s", owner ? owner : "unknown");
	s_visible_fb_enabled = fb_enabled;
	s_visible_fb_num = fb_num;
	s_visible_fb_width = fb_width;
	s_visible_fb_height = fb_height;
	mister_magik_status_write();
}

void mister_magik_note_osd_suppressed(void)
{
	s_osd_suppressed_count++;
	mister_magik_status_write();
}

static bool analytics_enabled(void)
{
	return access(s_analytics_flag_path, F_OK) == 0;
}

static void sanitize_detail(char *s)
{
	for (; *s; s++)
	{
		if (*s == '\t' || *s == '\n' || *s == '\r')
			*s = ' ';
	}
}

static void read_trimmed(const char *path, char *buf, size_t len)
{
	if (!buf || !len) return;
	buf[0] = 0;

	FILE *f = fopen(path, "r");
	if (!f) return;
	if (fgets(buf, len, f))
	{
		size_t n = strlen(buf);
		while (n && (buf[n - 1] == '\n' || buf[n - 1] == '\r' || buf[n - 1] == '\t' || buf[n - 1] == ' '))
			buf[--n] = 0;
		sanitize_detail(buf);
	}
	fclose(f);
}

static void analytics_event(const char *event, const char *fmt, ...)
{
	char detail[768];
	detail[0] = 0;
	if (fmt)
	{
		va_list args;
		va_start(args, fmt);
		vsnprintf(detail, sizeof(detail), fmt, args);
		va_end(args);
		detail[sizeof(detail) - 1] = 0;
		sanitize_detail(detail);
	}
	event_jsonl("main", event, detail);
	mister_magik_status_write();

	if (!analytics_enabled())
		return;

	FILE *f = fopen(s_analytics_path, "a");
	if (!f) return;

	if (!s_analytics_header_written)
	{
		fprintf(f, "seq\tsource\tboot_ms\tevent\tpid\tdetails\n");
		s_analytics_header_written = true;
	}

	fprintf(f, "%lu\tmain\t%lu\t%s\t%d\t%s\n",
	        ++s_analytics_seq, GetTimer(0), event, getpid(), detail);
	fclose(f);
}

static void analytics_state(const char *event, const char *extra_fmt = NULL, ...)
{
	video_boot_analytics_snapshot(event);

	char fb_mode[128];
	char active_vt[64];
	read_trimmed("/sys/module/MiSTer_fb/parameters/mode", fb_mode, sizeof(fb_mode));
	read_trimmed("/sys/class/tty/tty0/active", active_vt, sizeof(active_vt));

	char extra[384];
	extra[0] = 0;
	if (extra_fmt)
	{
		va_list args;
		va_start(args, extra_fmt);
		vsnprintf(extra, sizeof(extra), extra_fmt, args);
		va_end(args);
		extra[sizeof(extra) - 1] = 0;
		sanitize_detail(extra);
	}

	analytics_event(event, "pid=%d crash_count=%d respawn_timer=%lu init_pending=%d gave_up=%d escaped=%d tty_ready=%d active_vt=%s fb_mode=%s%s%s",
	                s_pid, s_crash_count, s_respawn_timer, s_init_pending, s_gave_up,
	                s_escaped, s_pid ? launcher_tty_ready(s_pid) : 0,
	                active_vt[0] ? active_vt : "unknown",
	                fb_mode[0] ? fb_mode : "unknown",
	                extra[0] ? " " : "", extra);
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
	analytics_state("return_to_normal_mode_start");
	user_io_osd_key_enable(1);
	reset_launcher_tty();
	video_fb_enable(0);
	video_menu_bg(user_io_status_get("[3:1]"));
	s_respawn_timer = 0;
	s_crash_count = 0;
	s_gave_up = true;
	s_escaped = true;
	log_msg("return_to_normal_mode");
	analytics_state("return_to_normal_mode_done");
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
	static const char analytics_cmd[] =
	    "#!/bin/bash\n"
	    "export LC_ALL=en_US.UTF-8\n"
	    "export HOME=/root\n"
	    "export MISTER_MAGIK_PARENT=main-mister\n"
	    "export MISTER_BOOT_ANALYTICS=1\n"
	    "export MISTER_PROFILE=summary\n"
	    "export MISTER_PROFILE_FILE=/tmp/mister-magik-frame-profile.tsv\n"
	    "export MISTER_BOOT_FRAME_PROFILE_FILE=/tmp/mister-magik-launcher-frame-profile.tsv\n"
	    "printf '\\033[0m\\033[?25l\\033[37m\\033[40m\\033[2J\\033[H'\n"
	    "exec \"$MISTER_MAGIK_PATH\" ui \"$MISTER_MAGIK_SCENE\" 0 >/tmp/mister-magik-slint.log 2>&1\n";

	unlink(s_script_path);
	const char *script = analytics_enabled() ? analytics_cmd : cmd;
	bool ok = FileSave(s_script_path, (void*)script, strlen(script)) != 0;
	analytics_event("write_script", "ok=%d analytics=%d path=%s", ok, analytics_enabled(), s_script_path);
	return ok;
}

static void spawn(void)
{
	analytics_state("spawn_start");
	char path[2100];
	strncpy(path, getFullPath(s_launcher_path), sizeof(path) - 1);
	path[sizeof(path) - 1] = '\0';

	if (!FileExists(s_launcher_path, 0))
	{
		analytics_event("spawn_missing_launcher", "path=%s", s_launcher_path);
		log_msg("spawn skipped: missing %s", s_launcher_path);
		return_to_normal_mode();
		return;
	}

	if (!write_launcher_script())
	{
		analytics_event("spawn_script_failed", "path=%s", s_script_path);
		log_msg("spawn failed: unable to write %s", s_script_path);
		return_to_normal_mode();
		return;
	}

	user_io_osd_key_enable(0);
	clear_launcher_tty();

	s_pid = fork();
	if (s_pid < 0)
	{
		analytics_event("fork_failed", "errno=%d error=%s", errno, strerror(errno));
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
	mister_magik_status_write();
	analytics_state("forked", "path=%s scene=%s", path, s_launcher_scene);
	wait_launcher_tty_ready(s_pid);
	analytics_state("tty_ready");
	video_chvt(s_vt);
	analytics_state("chvt_tty2");
	video_fb_enable(1);
	mister_magik_status_write();
	analytics_state("video_fb_enable_on");
	if (menu_present()) MenuHide();
	OsdDisable();
	analytics_state("menu_hide", "menu_present_after=%d", menu_present());
}

bool mister_magik_launcher_active(void)
{
	return s_pid != 0;
}

bool mister_magik_boot_analytics_enabled(void)
{
	return analytics_enabled();
}

void mister_magik_boot_analytics_event(const char *source, const char *event, const char *fmt, ...)
{
	char detail[768];
	detail[0] = 0;
	if (fmt)
	{
		va_list args;
		va_start(args, fmt);
		vsnprintf(detail, sizeof(detail), fmt, args);
		va_end(args);
		detail[sizeof(detail) - 1] = 0;
		sanitize_detail(detail);
	}
	event_jsonl(source ? source : "main", event, detail);
	mister_magik_status_write();

	if (!analytics_enabled())
		return;

	FILE *f = fopen(s_analytics_path, "a");
	if (!f) return;

	if (!s_analytics_header_written)
	{
		fprintf(f, "seq\tsource\tboot_ms\tevent\tpid\tdetails\n");
		s_analytics_header_written = true;
	}

	fprintf(f, "%lu\t%s\t%lu\t%s\t%d\t%s\n",
	        ++s_analytics_seq, source ? source : "main", GetTimer(0), event, getpid(), detail);
	fclose(f);
}

void mister_magik_launcher_init_for_menu(void)
{
	if (!mister_magik_launcher_configured() || s_pid || s_gave_up)
		return;

	s_crash_count = 0;
	s_respawn_timer = 0;
	s_init_pending = true;
	log_msg("init_for_menu");
	mister_magik_status_write();
	analytics_state("init_for_menu");
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
			mister_magik_status_write();
			analytics_state("launcher_exited", "escaped=%d crashed=%d status=%d sig=%d",
			                escaped, crashed, exit_status, sig);
			reset_launcher_tty();
			analytics_state("reset_launcher_tty");

			if (escaped)
			{
				return_to_normal_mode();
				return;
			}

			if (crashed && ++s_crash_count >= 3)
			{
				log_msg("giving up after 3 crashes");
				analytics_state("giving_up_after_crashes");
				return_to_normal_mode();
				return;
			}

			if (!crashed)
				s_crash_count = 0;

			s_respawn_timer = GetTimer(1000);
			if (!s_respawn_timer) s_respawn_timer = 1;
			analytics_state("respawn_scheduled");
		}
		return;
	}

	if (!mister_magik_launcher_configured())
		return;

	if (s_init_pending)
	{
		s_init_pending = false;
		analytics_state("init_pending_spawn");
		spawn();
		return;
	}

	if (s_respawn_timer && CheckTimer(s_respawn_timer))
	{
		s_respawn_timer = 0;
		analytics_state("respawn_timer_elapsed");
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
	mister_magik_status_write();
	analytics_state("shutdown");
}

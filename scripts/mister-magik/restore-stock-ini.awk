function finish_section() {
  if (section == "menu" && !wrote_menu_video) print "video_mode=8"
}

{
  sub(/\r$/, "", $0)
}

/^\[[^]]+\]$/ {
  finish_section()
  section = tolower(substr($0, 2, length($0) - 2))
  skip = 0

  if (section == "mister") {
    if (seen_mister) {
      section = "mister_duplicate"
      skip = 1
      next
    }
    seen_mister = 1
  } else if (section == "menu") {
    seen_menu = 1
    wrote_menu_video = 0
  }

  print
  next
}

skip { next }

section == "mister" && tolower($0) ~ /^[[:space:]]*main[[:space:]]*=[[:space:]]*mister_magik[[:space:]]*$/ {
  next
}

section == "menu" && tolower($0) ~ /^[[:space:]]*video_mode[[:space:]]*=/ {
  if (!wrote_menu_video) {
    print "video_mode=8"
    wrote_menu_video = 1
  }
  next
}

tolower($0) ~ /^[[:space:]]*vrr_(min|max)_framerate[[:space:]]*=/ {
  print ";" $0
  next
}

{ print }

END {
  finish_section()
  if (!seen_menu) {
    print ""
    print "[Menu]"
    print "video_mode=8"
  }
}

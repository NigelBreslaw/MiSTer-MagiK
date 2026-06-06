function finish_section() {
  if (section == "mister") {
    if (!wrote_direct) print "direct_video=0"
    if (!wrote_main) print "main=MiSTer_MagiK"
  }
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
    wrote_direct = 0
    wrote_main = 0
  } else if (section == "menu") {
    seen_menu = 1
    wrote_menu_video = 0
  }

  print
  next
}

skip { next }

section == "mister" && tolower($0) ~ /^[[:space:]]*direct_video[[:space:]]*=/ {
  if (!wrote_direct) {
    print "direct_video=0"
    wrote_direct = 1
  }
  next
}

section == "mister" && tolower($0) ~ /^[[:space:]]*main[[:space:]]*=/ {
  if (!wrote_main) {
    print "main=MiSTer_MagiK"
    wrote_main = 1
  }
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
  if (!seen_mister) {
    print "[MiSTer]"
    print "direct_video=0"
    print "main=MiSTer_MagiK"
  }
  if (!seen_menu) {
    print ""
    print "[Menu]"
    print "video_mode=8"
  }
}

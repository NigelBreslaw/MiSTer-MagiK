// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later
// Isolated production C kernels; run through the native run-benchmark-v2 lease.
#define _POSIX_C_SOURCE 200809L
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

void magik_launcher_project_dithered(uint16_t *,size_t,const uint32_t *,size_t,size_t,int32_t,int32_t,size_t,size_t);
void magik_launcher_project_dithered_opaque(uint16_t *,size_t,const uint32_t *,size_t,size_t,int32_t,int32_t,size_t,size_t,size_t,size_t);

#define WIDTH 960
#define HEIGHT 540
#define SOURCE_HEIGHT 273
#define COLUMNS 32
#define REPEATS 32
#define FIXTURE "057d9add0b5a0e6186bc8bcb630b3a32f17e734f36c2fec32e53efe42918ac4b"
static uint32_t source[COLUMNS][SOURCE_HEIGHT];
static uint16_t expected[WIDTH*HEIGHT],output[WIDTH*HEIGHT];
static const int32_t steps[]={40000,65536,98000,140000};
static const int32_t starts[]={-65536,0,7*65536,13*65536};

static uint64_t now(clockid_t clock) {
  struct timespec t;
  if(clock_gettime(clock,&t)) {perror("clock_gettime");exit(1);}
  return (uint64_t)t.tv_sec*1000000000u+(uint64_t)t.tv_nsec;
}
static size_t sweep(uint16_t *dest,int opaque) {
  size_t pixels=0;
  for(size_t s=0;s<4;++s)for(size_t q=0;q<4;++q) {
    size_t rows=(SOURCE_HEIGHT*65536-starts[q])/steps[s]+2;
    for(size_t x=0;x<COLUMNS;++x) {
      if(opaque)magik_launcher_project_dithered_opaque(dest+29*WIDTH+320+x,WIDTH,
        source[x],SOURCE_HEIGHT,rows,starts[q],steps[s],320+x,29,8,SOURCE_HEIGHT-8);
      else magik_launcher_project_dithered(dest+29*WIDTH+320+x,WIDTH,
        source[x],SOURCE_HEIGHT,rows,starts[q],steps[s],320+x,29);
    }
    pixels+=rows*COLUMNS;
  }
  return pixels;
}
int main(int argc,char **argv) {
  if(argc!=5 || strcmp(argv[1],"--bench") || strcmp(argv[3],"--mode") || strcmp(argv[4],"timing"))return 1;
  int opaque=!strcmp(argv[2],"card-column-opaque");
  if(!opaque && strcmp(argv[2],"card-column-generic"))return 1;
  const char *sha=getenv("MISTER_MAGIK2_ARTIFACT_SHA256");
  if(!sha || strlen(sha)!=64)return 1;
  for(size_t x=0;x<COLUMNS;++x)for(size_t y=0;y<SOURCE_HEIGHT;++y) {
    uint32_t a=y<8?y*31:y>=SOURCE_HEIGHT-8?(SOURCE_HEIGHT-1-y)*31:255;
    source[x][y]=((x*17+y*7)%(a+1))|(((x*13+y*3)%(a+1))<<8)|(((x*11+y*5)%(a+1))<<16)|(a<<24);
  }
  for(size_t i=0;i<WIDTH*HEIGHT;++i)expected[i]=output[i]=(uint16_t)(i*977);
  sweep(expected,0);size_t pixels=sweep(output,1);
  if(memcmp(expected,output,sizeof output)) {fputs("compose pixel mismatch\n",stderr);return 2;}
  uint64_t wall[2],cpu[2];
  for(size_t rep=0;rep<2;++rep) {
    // Warm both source and destination outside the measured interval.
    sweep(output,opaque);
    uint64_t c=now(CLOCK_THREAD_CPUTIME_ID),w=now(CLOCK_MONOTONIC);
    for(size_t i=0;i<REPEATS;++i)sweep(output,opaque);
    wall[rep]=now(CLOCK_MONOTONIC)-w;cpu[rep]=now(CLOCK_THREAD_CPUTIME_ID)-c;
  }
  pixels*=REPEATS;
  printf("{\"schema_version\":1,\"workload\":\"%s\",\"mode\":\"timing\",\"artifact_sha256\":\"%s\",\"correctness\":\"passed\",\"fixture\":{\"identity\":\"%s\",\"geometry\":[960,540],\"source_strip\":[32,273],\"profiles\":16,\"repeats\":32,\"timed_work\":\"projected dithered compose only\"},\"work_count\":%zu,\"samples\":[",argv[2],sha,FIXTURE,pixels);
  for(size_t rep=0;rep<2;++rep)printf("%s{\"repetition\":%zu,\"fixture_identity\":\"%s\",\"work_count\":%zu,\"duration_ns\":%llu,\"thread_cpu_ns\":%llu,\"ns_per_pixel\":%.12f}",rep?",":"",rep,FIXTURE,pixels,(unsigned long long)wall[rep],(unsigned long long)cpu[rep],(double)wall[rep]/pixels);
  printf("],\"checksum\":%u}\n",output[29*WIDTH+320]);
  return 0;
}

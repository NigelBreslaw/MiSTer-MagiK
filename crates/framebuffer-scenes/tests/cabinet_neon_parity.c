// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later
#include <stdio.h>
#include <string.h>
#include "../src/launcher_texture_neon.c"
static uint32_t seed=73;
static uint32_t next(void){seed=seed*1664525+1013904223;return seed;}
int main(void) {
 for(size_t trial=0;trial<100000;++trial) {
  uint32_t a[36],b[36],c[36],d[36];uint16_t old[38],new[38];size_t n=next()%37;
  for(size_t j=0;j<36;++j){a[j]=next();b[j]=next();c[j]=next();d[j]=next();}
  memset(old,0,sizeof old);memset(new,0,sizeof new);old[0]=new[0]=old[n+1]=new[n+1]=0x7364;
  uint32_t wy=next()%257,wy2=next()%257,lod=next()%257;size_t x=next()%960,y=next()%540;
  magik_cabinet_composite(old+1,a,b,c,d,n,wy,wy2,lod,x,y,0);
  magik_cabinet_composite(new+1,a,b,c,d,n,wy,wy2,lod,x,y,1);
  if(memcmp(old,new,sizeof old)) {fprintf(stderr,"black mismatch %zu\n",trial);return 1;}
 }
 puts("100000 known-black cabinet rows exactly match, including all weights, phases, tails and guards");
}

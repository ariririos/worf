main: main.c
	gcc -o main -lmpdclient -lsqlite3 -lm -lffcall -Wall -Werror -g main.c
# 	gcc -o main -lmpdclient -lsqlite3 -lm -lffcall -Wall -Werror -g -fsanitize=address -fno-omit-frame-pointer main.c
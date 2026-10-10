项目源自于EchoBird https://github.com/edison7009/EchoBird 感谢大佬的开源项目，主要是为了自己使用把手里各处搞到的key可以智能调度就把原项目进行了精简只留下了免费模型这一块
修改的时候基于EchoBird的5.7版，并对这一块进行了一些如下修改

1.改进智能路由均衡方式

2.添加速度限制免费的一般是20RPM，添加的时候看官方说明，不清楚就不填，付费的key不限速可以填-1

3.模型列表可以拖动排序。

4.修正添加模型的时候必须有OpenAI 地址的问题

5.双击已添加模型弹出添加时的对话框进行修改

6.去重升级修改配置文件保存位置等一些细节调整

个人感觉差不多够用了，有别的需求可以自行修改

界面如下

<img width="2826" height="1814" alt="c8911e12-c989-47a1-9e1b-b40d77390104" src="https://github.com/user-attachments/assets/eb7f721f-0e3d-457c-84d4-4e9f08d01978" />

手动编译方法
npx vite build

cd src-tauri

cargo build --release --features tauri/custom-protocol
